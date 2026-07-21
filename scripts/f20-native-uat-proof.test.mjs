// node --test suite for f20-native-uat-proof.mjs
//
// Proves the verifier non-vacuously: every positive path is exercised, and
// every authority check is driven to its fail-closed branch with a targeted
// tamper (a mutated byte, a symlink where a regular file is required, a
// missing/extra/duplicate/reordered marker, a conflicting idempotency tuple,
// a pre-existing or nonce-less run). No production push or dispatch runs.

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, symlinkSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  ProofError,
  readExactBytesNoFollow,
  parseExactLines,
  validatePublication,
  reconcileRequest,
  reconcileAuthorization,
  bindRun,
  verifyNativeLog,
  verifyNativeLogFile,
  WINDOWS_TARGETS,
  MACOS_TARGETS,
} from './f20-native-uat-proof.mjs';

const COMMIT = 'a'.repeat(40);
const TREE = 'b'.repeat(40);
const NONCE = 'c'.repeat(32);

function scratch() {
  const dir = mkdtempSync(join(tmpdir(), 'f20-uat-'));
  return dir;
}

function winLog({ commit = COMMIT, tree = TREE, nonce = NONCE, targets = WINDOWS_TARGETS, withFinal = true } = {}) {
  const lines = targets.map(
    (t) => `F20_NATIVE_TARGET=PASS platform=windows target=${t} commit=${commit} tree=${tree} nonce=${nonce}`,
  );
  if (withFinal) lines.push(`F20_NATIVE_WINDOWS_ACCEPTANCE=PASS commit=${commit} tree=${tree} nonce=${nonce}`);
  return lines.join('\n') + '\n';
}

function macLog() {
  const lines = MACOS_TARGETS.map(
    (t) => `F20_NATIVE_TARGET=PASS platform=macos target=${t} commit=${COMMIT} tree=${TREE} nonce=${NONCE}`,
  );
  lines.push(`F20_NATIVE_MACOS_ACCEPTANCE=PASS commit=${COMMIT} tree=${TREE} nonce=${NONCE}`);
  return lines.join('\n') + '\n';
}

// ---- no-follow exact-byte reader ------------------------------------------

test('readExactBytesNoFollow returns exact bytes of a regular file', () => {
  const dir = scratch();
  const p = join(dir, 'log.txt');
  const payload = winLog();
  writeFileSync(p, payload);
  const bytes = readExactBytesNoFollow(p);
  assert.equal(bytes.toString('utf8'), payload);
});

test('readExactBytesNoFollow refuses a symlink where a real file is required', () => {
  const dir = scratch();
  const real = join(dir, 'real.txt');
  const link = join(dir, 'link.txt');
  writeFileSync(real, winLog());
  symlinkSync(real, link);
  assert.throws(() => readExactBytesNoFollow(link), ProofError);
});

test('readExactBytesNoFollow refuses a directory (non-regular file)', () => {
  const dir = scratch();
  const sub = join(dir, 'sub');
  mkdirSync(sub);
  assert.throws(() => readExactBytesNoFollow(sub), (e) => e instanceof ProofError || e.code === 'EISDIR');
});

test('readExactBytesNoFollow reports a missing artifact', () => {
  const dir = scratch();
  assert.throws(() => readExactBytesNoFollow(join(dir, 'nope.txt')), ProofError);
});

test('verifyNativeLogFile reads through the no-follow reader and passes', () => {
  const dir = scratch();
  const p = join(dir, 'win.log');
  writeFileSync(p, winLog());
  const res = verifyNativeLogFile(p, { platform: 'windows', commit: COMMIT, tree: TREE, nonce: NONCE });
  assert.equal(res.targets.length, WINDOWS_TARGETS.length);
});

test('verifyNativeLogFile refuses a symlinked log', () => {
  const dir = scratch();
  const real = join(dir, 'r.log');
  const link = join(dir, 'l.log');
  writeFileSync(real, winLog());
  symlinkSync(real, link);
  assert.throws(
    () => verifyNativeLogFile(link, { platform: 'windows', commit: COMMIT, tree: TREE, nonce: NONCE }),
    ProofError,
  );
});

// ---- newline grammar -------------------------------------------------------

test('parseExactLines rejects a CR byte', () => {
  assert.throws(() => parseExactLines(Buffer.from('a\r\n')), ProofError);
});

test('parseExactLines rejects a missing final newline', () => {
  assert.throws(() => parseExactLines(Buffer.from('a')), ProofError);
});

test('parseExactLines rejects a blank line', () => {
  assert.throws(() => parseExactLines(Buffer.from('a\n\nb\n')), ProofError);
});

// ---- native-log marker verification ---------------------------------------

test('verifyNativeLog accepts a complete ordered windows log', () => {
  const res = verifyNativeLog(Buffer.from(winLog()), {
    platform: 'windows',
    commit: COMMIT,
    tree: TREE,
    nonce: NONCE,
  });
  assert.deepEqual(res.targets, WINDOWS_TARGETS);
});

test('verifyNativeLog accepts a complete macOS log', () => {
  const res = verifyNativeLog(Buffer.from(macLog()), {
    platform: 'macos',
    commit: COMMIT,
    tree: TREE,
    nonce: NONCE,
  });
  assert.deepEqual(res.targets, MACOS_TARGETS);
});

test('verifyNativeLog rejects a single mutated byte in a marker', () => {
  const bytes = Buffer.from(winLog());
  // Flip one hex char of the commit in the first marker line.
  const idx = bytes.indexOf('commit=' + COMMIT) + 'commit='.length;
  bytes[idx] = bytes[idx] === 0x61 ? 0x62 : 0x61; // a<->b
  assert.throws(
    () => verifyNativeLog(bytes, { platform: 'windows', commit: COMMIT, tree: TREE, nonce: NONCE }),
    ProofError,
  );
});

test('verifyNativeLog rejects a missing target marker', () => {
  const partial = winLog({ targets: WINDOWS_TARGETS.slice(0, -1) });
  assert.throws(
    () => verifyNativeLog(Buffer.from(partial), { platform: 'windows', commit: COMMIT, tree: TREE, nonce: NONCE }),
    ProofError,
  );
});

test('verifyNativeLog rejects a duplicate target marker', () => {
  const dup = winLog({ targets: [...WINDOWS_TARGETS, WINDOWS_TARGETS[0]] });
  assert.throws(
    () => verifyNativeLog(Buffer.from(dup), { platform: 'windows', commit: COMMIT, tree: TREE, nonce: NONCE }),
    ProofError,
  );
});

test('verifyNativeLog rejects reordered target markers', () => {
  const reordered = [WINDOWS_TARGETS[1], WINDOWS_TARGETS[0], ...WINDOWS_TARGETS.slice(2)];
  assert.throws(
    () => verifyNativeLog(Buffer.from(winLog({ targets: reordered })), {
      platform: 'windows',
      commit: COMMIT,
      tree: TREE,
      nonce: NONCE,
    }),
    ProofError,
  );
});

test('verifyNativeLog rejects a foreign/extra target marker', () => {
  const extra = winLog({ targets: [...WINDOWS_TARGETS, 'windows-not-a-real-target'] });
  assert.throws(
    () => verifyNativeLog(Buffer.from(extra), { platform: 'windows', commit: COMMIT, tree: TREE, nonce: NONCE }),
    ProofError,
  );
});

test('verifyNativeLog rejects a missing final acceptance marker', () => {
  const noFinal = winLog({ withFinal: false });
  assert.throws(
    () => verifyNativeLog(Buffer.from(noFinal), { platform: 'windows', commit: COMMIT, tree: TREE, nonce: NONCE }),
    ProofError,
  );
});

test('verifyNativeLog rejects a duplicate final acceptance marker', () => {
  const doubled = winLog() + `F20_NATIVE_WINDOWS_ACCEPTANCE=PASS commit=${COMMIT} tree=${TREE} nonce=${NONCE}\n`;
  assert.throws(
    () => verifyNativeLog(Buffer.from(doubled), { platform: 'windows', commit: COMMIT, tree: TREE, nonce: NONCE }),
    ProofError,
  );
});

test('verifyNativeLog rejects a nonce drift between targets and expectation', () => {
  assert.throws(
    () => verifyNativeLog(Buffer.from(winLog()), {
      platform: 'windows',
      commit: COMMIT,
      tree: TREE,
      nonce: 'd'.repeat(32),
    }),
    ProofError,
  );
});

// ---- publication object ----------------------------------------------------

function publication(overrides = {}) {
  return {
    kind: 'publication',
    candidate: 'f20-08',
    full_sha: COMMIT,
    tree: TREE,
    ref: `refs/f20-native-uat/${COMMIT}`,
    workflow: 'nightly-windows-soak.yml',
    ...overrides,
  };
}

test('validatePublication accepts a well-formed publication', () => {
  assert.doesNotThrow(() => validatePublication(publication(), { candidate: 'f20-08', fullSha: COMMIT, tree: TREE }));
});

test('validatePublication rejects a ref that does not embed the SHA', () => {
  assert.throws(() => validatePublication(publication({ ref: `refs/f20-native-uat/${'e'.repeat(40)}` })), ProofError);
});

test('validatePublication rejects a non-UAT ref namespace', () => {
  assert.throws(() => validatePublication(publication({ ref: 'refs/heads/main' })), ProofError);
});

test('validatePublication rejects a tree drift', () => {
  assert.throws(() => validatePublication(publication(), { tree: 'f'.repeat(40) }), ProofError);
});

// ---- request / authorization idempotency ----------------------------------

function request(overrides = {}) {
  return {
    kind: 'request',
    status: 'pending',
    candidate: 'f20-08',
    commit: COMMIT,
    tree: TREE,
    ref: `refs/f20-native-uat/${COMMIT}`,
    runner_label: 'f20-native-macos',
    image_label: 'f20-image-' + '9'.repeat(64),
    nonce: NONCE,
    ...overrides,
  };
}

test('reconcileRequest returns the existing object for an identical tuple', () => {
  const existing = request();
  const out = reconcileRequest(existing, request());
  assert.equal(out, existing);
});

test('reconcileRequest fails closed on a conflicting tuple', () => {
  const existing = request();
  assert.throws(() => reconcileRequest(existing, request({ nonce: 'd'.repeat(32) })), ProofError);
});

test('reconcileRequest fails closed on same tuple but drifted commit', () => {
  const existing = request();
  assert.throws(() => reconcileRequest(existing, request({ commit: 'e'.repeat(40) })), ProofError);
});

test('reconcileRequest rejects a non-pending existing object', () => {
  assert.throws(() => reconcileRequest(request({ status: 'authorized' }), request()), ProofError);
});

test('reconcileAuthorization is exact-response idempotent', () => {
  const req = request();
  const digest = '1'.repeat(64);
  const first = reconcileAuthorization(null, req, digest);
  const second = reconcileAuthorization(first, req, digest);
  assert.equal(first, second);
});

test('reconcileAuthorization fails closed on digest drift', () => {
  const req = request();
  const first = reconcileAuthorization(null, req, '1'.repeat(64));
  assert.throws(() => reconcileAuthorization(first, req, '2'.repeat(64)), ProofError);
});

// ---- run binding -----------------------------------------------------------

const REF = `refs/f20-native-uat/${COMMIT}`;

function candidateRun(overrides = {}) {
  return {
    run_id: 'run-100',
    created_at: 2000,
    nonce: NONCE,
    source_sha: COMMIT,
    ref: REF,
    runner_id: 'runner-7',
    runner_name: 'f20-mac-ephemeral',
    ...overrides,
  };
}

test('bindRun binds exactly one post-boundary nonce-carrying run', () => {
  const run = bindRun({
    candidateRuns: [candidateRun()],
    preExistingRunIds: [],
    apiTimeBoundary: 1000,
    nonce: NONCE,
    sourceSha: COMMIT,
    ref: REF,
  });
  assert.equal(run.run_id, 'run-100');
});

test('bindRun rejects a pre-existing run id', () => {
  assert.throws(
    () =>
      bindRun({
        candidateRuns: [candidateRun()],
        preExistingRunIds: ['run-100'],
        apiTimeBoundary: 1000,
        nonce: NONCE,
        sourceSha: COMMIT,
        ref: REF,
      }),
    ProofError,
  );
});

test('bindRun fails closed when zero runs carry the nonce', () => {
  assert.throws(
    () =>
      bindRun({
        candidateRuns: [candidateRun({ nonce: 'd'.repeat(32) })],
        preExistingRunIds: [],
        apiTimeBoundary: 1000,
        nonce: NONCE,
        sourceSha: COMMIT,
        ref: REF,
      }),
    ProofError,
  );
});

test('bindRun fails closed on ambiguous multiple matches', () => {
  assert.throws(
    () =>
      bindRun({
        candidateRuns: [candidateRun(), candidateRun({ run_id: 'run-101' })],
        preExistingRunIds: [],
        apiTimeBoundary: 1000,
        nonce: NONCE,
        sourceSha: COMMIT,
        ref: REF,
      }),
    ProofError,
  );
});

test('bindRun rejects a run created before the API time boundary', () => {
  assert.throws(
    () =>
      bindRun({
        candidateRuns: [candidateRun({ created_at: 500 })],
        preExistingRunIds: [],
        apiTimeBoundary: 1000,
        nonce: NONCE,
        sourceSha: COMMIT,
        ref: REF,
      }),
    ProofError,
  );
});
