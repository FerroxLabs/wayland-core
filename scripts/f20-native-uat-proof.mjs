// f20-native-uat-proof.mjs
//
// Sole terminal-plan (20-17/20-18) publication / state / run / native-log
// authority for the Phase 20 native UAT. This module is CONSTRUCTED here in
// plan 20-08 and its verifier logic is proved by f20-native-uat-proof.test.mjs.
// It performs NO push and NO workflow dispatch by itself — those external
// mutations remain Sean-gated at the terminal plan. This file provides the
// pure verification primitives that a terminal run composes, the no-follow
// exact-byte reader those primitives require, AND the sole durable request
// WRITER (persistRequest / the `request` subcommand): the only path that
// persists the pending request the verifier later reads, giving writer/verifier
// parity. The writer's only side effect is persisting that Git-private pending
// request; it still performs no push, dispatch, commit, format, or Cargo run.
//
// Design invariants (see 20-08-PLAN.md Task 3 <behavior>):
//   * Every authority/state object is opened exactly once through a
//     no-follow file descriptor, its regular-file identity is confirmed via
//     fstat, its exact bytes are retained in memory, and only those retained
//     bytes flow into later authority checks. A pathname is never reopened.
//   * Symlinks, FIFOs, directories, and other non-regular objects at an
//     authority path fail closed.
//   * Newline grammar is exact: LF-terminated lines, no CR bytes, a single
//     trailing newline, no blank/needle-injected lines.
//   * Request and authorization operations are exact-tuple idempotent:
//     repeating the identical tuple returns the existing object; any
//     conflicting or malformed/non-pending object fails closed.
//   * Native-log verification counts each required target marker exactly
//     once, in order, all bound to the same candidate commit/tree/nonce,
//     followed by exactly one final platform acceptance marker.
//
// No Cargo, format, commit, push, or dispatch action is performed on import or
// by any exported function; the request writer's sole effect is persisting the
// Git-private pending request. Callers in the terminal plan own push/dispatch.

import {
  openSync,
  fstatSync,
  readSync,
  writeSync,
  fsyncSync,
  closeSync,
  mkdirSync,
  realpathSync,
  constants,
} from 'node:fs';
import { execFileSync } from 'node:child_process';
import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const HEX40_OR_64 = /^[0-9a-f]{40}([0-9a-f]{24})?$/;
const NONCE_RE = /^[0-9a-f]{32,64}$/;
const UAT_REF_RE = /^refs\/f20-native-uat\/[0-9a-f]{40}([0-9a-f]{24})?$/;

// O_NOFOLLOW is present on Linux and macOS. On platforms lacking it we fall
// back to O_RDONLY and rely on the post-open fstat regular-file check; the
// terminal UAT only ever runs on Linux/macOS hosts.
const NOFOLLOW = constants.O_NOFOLLOW ?? 0;

export class ProofError extends Error {
  constructor(message) {
    super(message);
    this.name = 'ProofError';
  }
}

function fail(message) {
  throw new ProofError(message);
}

// Open `path` exactly once with no-follow semantics, confirm it is a regular
// file via the SAME descriptor's fstat (never a second pathname stat, which
// would be TOCTOU-racy), read its entire contents, and return the retained
// bytes. The descriptor is always closed. A symlink at `path` makes the
// open() itself fail with ELOOP under O_NOFOLLOW; on fallback platforms the
// fstat regular-file check rejects anything that is not a plain file.
export function readExactBytesNoFollow(path) {
  let fd;
  try {
    fd = openSync(path, constants.O_RDONLY | NOFOLLOW);
  } catch (err) {
    if (err && (err.code === 'ELOOP' || err.code === 'EMLINK')) {
      fail(`refused to follow symlink at authority path: ${path}`);
    }
    if (err && err.code === 'ENOENT') {
      fail(`missing authority artifact: ${path}`);
    }
    throw err;
  }
  try {
    const st = fstatSync(fd);
    if (!st.isFile()) {
      fail(`authority path is not a regular file: ${path}`);
    }
    const size = st.size;
    const buf = Buffer.allocUnsafe(size);
    let read = 0;
    while (read < size) {
      const n = readSync(fd, buf, read, size - read, read);
      if (n === 0) break;
      read += n;
    }
    if (read !== size) {
      fail(`short read on authority artifact: ${path}`);
    }
    // A second fstat on the same fd guards against a size change between the
    // stat and the completed read (fail closed rather than trust a partial).
    const st2 = fstatSync(fd);
    if (st2.size !== size) {
      fail(`authority artifact changed size during read: ${path}`);
    }
    return buf;
  } finally {
    closeSync(fd);
  }
}

// Parse retained bytes as strict LF-terminated UTF-8 lines. Rejects CR bytes,
// a missing final newline, and blank lines (which could smuggle a duplicate
// or reordered marker past a naive line filter).
export function parseExactLines(bytes) {
  if (!Buffer.isBuffer(bytes)) fail('expected retained Buffer');
  if (bytes.length === 0) fail('empty authority artifact');
  if (bytes.includes(0x0d)) fail('CR byte in authority artifact (LF grammar required)');
  if (bytes[bytes.length - 1] !== 0x0a) fail('authority artifact missing final newline');
  const text = bytes.toString('utf8');
  const lines = text.slice(0, -1).split('\n');
  for (const line of lines) {
    if (line.length === 0) fail('blank line in authority artifact');
  }
  return lines;
}

// Parse retained bytes as a single JSON object (state/publication/run objects
// are canonical single-line JSON with a trailing newline).
export function parseJsonObject(bytes) {
  const lines = parseExactLines(bytes);
  if (lines.length !== 1) fail('state object must be exactly one JSON line');
  let obj;
  try {
    obj = JSON.parse(lines[0]);
  } catch {
    fail('state object is not valid JSON');
  }
  if (obj === null || typeof obj !== 'object' || Array.isArray(obj)) {
    fail('state object must be a JSON object');
  }
  return obj;
}

function expectHex(value, label) {
  if (typeof value !== 'string' || !HEX40_OR_64.test(value)) {
    fail(`${label} must be lowercase 40- or 64-hex`);
  }
}

// ---- UAT publication object ------------------------------------------------
// Shape: { kind:"publication", candidate, full_sha, tree, ref, workflow }
export function validatePublication(obj, { candidate, fullSha, tree } = {}) {
  if (obj.kind !== 'publication') fail('publication kind mismatch');
  expectHex(obj.full_sha, 'publication.full_sha');
  expectHex(obj.tree, 'publication.tree');
  if (typeof obj.candidate !== 'string' || obj.candidate.length === 0) {
    fail('publication.candidate required');
  }
  if (typeof obj.ref !== 'string' || !UAT_REF_RE.test(obj.ref)) {
    fail('publication.ref must be the exact refs/f20-native-uat/<sha> ref');
  }
  if (typeof obj.workflow !== 'string' || obj.workflow.length === 0) {
    fail('publication.workflow required');
  }
  if (!obj.ref.endsWith(obj.full_sha)) {
    fail('publication.ref must embed the exact full SHA');
  }
  if (candidate !== undefined && obj.candidate !== candidate) fail('publication candidate drift');
  if (fullSha !== undefined && obj.full_sha !== fullSha) fail('publication SHA drift');
  if (tree !== undefined && obj.tree !== tree) fail('publication tree drift');
  return obj;
}

// ---- Request / authorization state (exact-tuple idempotent) ----------------
const REQUEST_TUPLE = ['candidate', 'ref', 'runner_label', 'image_label', 'nonce'];

export function validateRequest(obj) {
  if (obj.kind !== 'request') fail('request kind mismatch');
  if (obj.status !== 'pending') fail('request must be pending');
  expectHex(obj.commit, 'request.commit');
  expectHex(obj.tree, 'request.tree');
  if (typeof obj.ref !== 'string' || obj.ref.length === 0) fail('request.ref required');
  if (typeof obj.runner_label !== 'string' || obj.runner_label.length === 0) {
    fail('request.runner_label required');
  }
  if (typeof obj.image_label !== 'string' || !obj.image_label.startsWith('f20-image-')) {
    fail('request.image_label must be f20-image-<sha256>');
  }
  if (typeof obj.nonce !== 'string' || !NONCE_RE.test(obj.nonce)) fail('request.nonce required');
  if (typeof obj.candidate !== 'string' || obj.candidate.length === 0) fail('request.candidate required');
  return obj;
}

function sameTuple(a, b, keys) {
  return keys.every((k) => a[k] === b[k]);
}

// Exact-tuple idempotent request creation. If an existing pending object
// carries the identical request tuple, return it unchanged. A conflicting
// tuple, or an existing non-pending / malformed object, fails closed.
export function reconcileRequest(existing, requested) {
  validateRequest(requested);
  if (existing === null || existing === undefined) return requested;
  validateRequest(existing);
  if (!sameTuple(existing, requested, REQUEST_TUPLE)) {
    fail('conflicting request tuple for existing pending object');
  }
  // Non-tuple authority fields must also match to be considered the same request.
  if (existing.commit !== requested.commit || existing.tree !== requested.tree) {
    fail('conflicting commit/tree for identical request tuple');
  }
  return existing;
}

// ---- Durable request WRITER path (writer/verifier parity) ------------------
// The primitives above READ a persisted pending request; the functions below
// are the SOLE writer that PERSISTS it, closing the writer/verifier gap so
// terminal-plan preparation (20-27) durably writes exactly the pending tuple
// terminal-plan authentication (20-28) later reads. No push and no dispatch is
// performed here — only the pending request the human-authorized terminal plan
// consumes is persisted.

// Canonical single-line JSON (fixed field order) + trailing newline. Fixed
// ordering makes an exact-tuple re-request produce byte-identical content, so
// idempotent re-writes never perturb the persisted authority bytes.
const REQUEST_FIELDS = ['kind', 'status', 'candidate', 'commit', 'tree', 'ref', 'runner_label', 'image_label', 'nonce'];

export function serializeRequest(obj) {
  validateRequest(obj);
  const canonical = {};
  for (const key of REQUEST_FIELDS) canonical[key] = obj[key];
  return JSON.stringify(canonical) + '\n';
}

// Durably persist canonical `text` to `path` with the same no-follow +
// regular-file discipline as the reader, mode 0600, and an fsync before close
// so a crash cannot strand a torn or absent pending request. A symlink at the
// final path component fails closed (ELOOP under O_NOFOLLOW; the post-open
// fstat regular-file check backstops fallback platforms lacking O_NOFOLLOW).
export function writeExactBytesNoFollow(path, text) {
  const buf = Buffer.from(text, 'utf8');
  let fd;
  try {
    fd = openSync(path, constants.O_WRONLY | constants.O_CREAT | constants.O_TRUNC | NOFOLLOW, 0o600);
  } catch (err) {
    if (err && (err.code === 'ELOOP' || err.code === 'EMLINK')) {
      fail(`refused to follow symlink at authority path: ${path}`);
    }
    throw err;
  }
  try {
    const st = fstatSync(fd);
    if (!st.isFile()) fail(`authority path is not a regular file: ${path}`);
    let written = 0;
    while (written < buf.length) {
      written += writeSync(fd, buf, written, buf.length - written);
    }
    fsyncSync(fd);
  } finally {
    closeSync(fd);
  }
}

// The request path: durably persist the exact pending native-proof tuple the
// verifier / proof-checkout paths later read. Exact-tuple idempotent — an
// identical prior pending request is returned unchanged and re-serialized to
// byte-identical canonical bytes; a conflicting, malformed, or non-pending
// existing object fails closed (via reconcileRequest / validateRequest). A
// symlink or non-regular object at the authority path fails closed. Returns the
// persisted pending object.
export function persistRequest(path, requested) {
  validateRequest(requested);
  let existing = null;
  try {
    existing = parseJsonObject(readExactBytesNoFollow(path));
  } catch (err) {
    if (err instanceof ProofError && err.message.startsWith('missing authority artifact')) {
      existing = null; // first write for this candidate — no prior pending object
    } else {
      throw err; // symlink / non-regular / malformed existing object fails closed
    }
  }
  const reconciled = reconcileRequest(existing, requested);
  writeExactBytesNoFollow(path, serializeRequest(reconciled));
  return reconciled;
}

// Exact-response idempotent authorization. Authorizing the same pending
// request twice yields the identical authorization digest; a different
// digest for the same request fails closed.
export function reconcileAuthorization(existingAuth, request, digest) {
  validateRequest(request);
  if (typeof digest !== 'string' || !HEX40_OR_64.test(digest)) {
    fail('authorization digest must be lowercase 40- or 64-hex');
  }
  if (existingAuth === null || existingAuth === undefined) {
    return { kind: 'authorization', nonce: request.nonce, digest };
  }
  if (existingAuth.kind !== 'authorization') fail('authorization kind mismatch');
  if (existingAuth.nonce !== request.nonce) fail('authorization bound to a different nonce');
  if (existingAuth.digest !== digest) fail('authorization digest drift for same request');
  return existingAuth;
}

// ---- Run binding -----------------------------------------------------------
// Exactly one post-boundary, not-pre-existing run carrying the nonce and the
// exact source/ref may be bound. Zero matches or more than one match fail
// closed; a run whose id was already present before the API time boundary is
// rejected (it cannot be "our" dispatch).
export function bindRun({ candidateRuns, preExistingRunIds, apiTimeBoundary, nonce, sourceSha, ref }) {
  if (typeof nonce !== 'string' || !NONCE_RE.test(nonce)) fail('bindRun requires a valid nonce');
  expectHex(sourceSha, 'bindRun.sourceSha');
  const pre = new Set(preExistingRunIds ?? []);
  const matches = [];
  for (const run of candidateRuns ?? []) {
    if (pre.has(run.run_id)) continue; // pre-existing → never ours
    if (typeof run.created_at !== 'number' || run.created_at < apiTimeBoundary) continue;
    if (run.nonce !== nonce) continue;
    if (run.source_sha !== sourceSha) continue;
    if (run.ref !== ref) continue;
    matches.push(run);
  }
  if (matches.length === 0) fail('no post-boundary run carrying the nonce and source/ref');
  if (matches.length > 1) fail('ambiguous run binding: more than one candidate run matched');
  const run = matches[0];
  if (typeof run.runner_id !== 'string' || run.runner_id.length === 0) fail('run.runner_id required');
  if (typeof run.runner_name !== 'string' || run.runner_name.length === 0) fail('run.runner_name required');
  return run;
}

// ---- Native-log marker verification ----------------------------------------
const TARGET_LINE_RE =
  /^F20_NATIVE_TARGET=PASS platform=(windows|macos) target=([a-z0-9-]+) commit=([0-9a-f]{40}(?:[0-9a-f]{24})?) tree=([0-9a-f]{40}(?:[0-9a-f]{24})?) nonce=([0-9a-f]{32,64})$/;

export const WINDOWS_TARGETS = [
  'windows-retained-handle',
  'windows-appcontainer-acl',
  'windows-job-object',
  'windows-public-dispatch',
  'windows-hard-process-containment',
  'windows-f20-lifecycle',
];

export const MACOS_TARGETS = [
  'macos-retained-directory',
  'macos-process-tree',
  'macos-docker-reject-path-replacement',
  'macos-docker-roundtrip-delete',
  'macos-public-dispatch',
  'macos-docker-cancellation',
  'macos-docker-budget',
  'macos-f20-lifecycle',
];

// Canonical native-target -> { crate, test, os } expectation (REQ-native-r8).
// This is the SHARED source of truth for the wrong-OS anti-drift guard: the
// PowerShell guard in f20-native-windows-proof.ps1 mirrors these `os` fields,
// and the macOS guard reuses MACOS_TARGET_SOURCES (applied in 20-22). It also
// records the 20-21 repoint (REQ-native-r7): windows-job-object and
// windows-hard-process-containment now select the REAL Windows Job-Object tests
// in crates/wcore-sandbox/tests/hard_process_containment_windows.rs, never the
// Linux-only Bubblewrap hard_process_containment.rs they were mis-wired to.
//
// os semantics:
//   'windows'/'macos' — an OS-specific native test; its source MUST be cfg-gated
//                       for that OS (the guard fails closed otherwise).
//   'any'             — a legitimately cross-platform test (no OS-exclusive gate);
//                       exempt from the positive-gate requirement.
export const WINDOWS_TARGET_SOURCES = {
  'windows-retained-handle': { crate: 'wcore-sandbox', test: 'live_fs_acl', os: 'windows' },
  'windows-appcontainer-acl': { crate: 'wcore-sandbox', test: 'live_fs_acl', os: 'windows' },
  'windows-job-object': { crate: 'wcore-sandbox', test: 'hard_process_containment_windows', os: 'windows' },
  'windows-public-dispatch': { crate: 'wcore-swarm', test: 'dispatch_smoke', os: 'any' },
  'windows-hard-process-containment': { crate: 'wcore-sandbox', test: 'hard_process_containment_windows', os: 'windows' },
  'windows-f20-lifecycle': { crate: 'wcore-agent', test: 'transactional_delegated_mutation_test', os: 'any' },
};

export const MACOS_TARGET_SOURCES = {
  'macos-retained-directory': { crate: 'wcore-sandbox', test: 'live_integrity_macos', os: 'macos' },
  'macos-process-tree': { crate: 'wcore-sandbox', test: 'hard_process_containment_macos', os: 'macos' },
  'macos-docker-reject-path-replacement': { crate: 'wcore-sandbox', test: 'docker_smoke', os: 'any' },
  'macos-docker-roundtrip-delete': { crate: 'wcore-sandbox', test: 'docker_smoke', os: 'any' },
  'macos-public-dispatch': { crate: 'wcore-swarm', test: null, os: 'any' },
  'macos-docker-cancellation': { crate: 'wcore-sandbox', test: 'docker_smoke', os: 'any' },
  'macos-docker-budget': { crate: 'wcore-swarm', test: 'workspace_authority', os: 'any' },
  'macos-f20-lifecycle': { crate: 'wcore-agent', test: 'transactional_delegated_mutation_test', os: 'any' },
};

const FINAL_MARKER = {
  windows: 'F20_NATIVE_WINDOWS_ACCEPTANCE=PASS',
  macos: 'F20_NATIVE_MACOS_ACCEPTANCE=PASS',
};

// Verify the retained bytes of a native job log. Each required target must
// appear exactly once, in the declared order, all bound to the same
// platform/commit/tree/nonce. Exactly one final platform acceptance marker
// (carrying the same commit/tree/nonce) must follow all target markers. Any
// absent, duplicate, reordered, foreign, or pre-final target marker — or a
// missing/duplicate/pre-target final marker — fails closed.
export function verifyNativeLog(bytes, { platform, commit, tree, nonce }) {
  const required = platform === 'windows' ? WINDOWS_TARGETS : platform === 'macos' ? MACOS_TARGETS : null;
  if (required === null) fail(`unknown platform: ${platform}`);
  expectHex(commit, 'commit');
  expectHex(tree, 'tree');
  if (!NONCE_RE.test(nonce ?? '')) fail('nonce required');

  const lines = parseExactLines(bytes);
  const seenTargets = [];
  let finalSeen = false;
  const finalLine = `${FINAL_MARKER[platform]} commit=${commit} tree=${tree} nonce=${nonce}`;

  for (const line of lines) {
    const m = TARGET_LINE_RE.exec(line);
    if (m) {
      if (finalSeen) fail('target marker after final acceptance marker');
      const [, mPlatform, target, mCommit, mTree, mNonce] = m;
      if (mPlatform !== platform) fail(`foreign platform marker: ${mPlatform}`);
      if (mCommit !== commit) fail(`target ${target} commit drift`);
      if (mTree !== tree) fail(`target ${target} tree drift`);
      if (mNonce !== nonce) fail(`target ${target} nonce drift`);
      if (!required.includes(target)) fail(`foreign target marker: ${target}`);
      if (seenTargets.includes(target)) fail(`duplicate target marker: ${target}`);
      seenTargets.push(target);
      continue;
    }
    if (line.startsWith(FINAL_MARKER[platform])) {
      if (line !== finalLine) fail('final acceptance marker does not bind exact commit/tree/nonce');
      if (finalSeen) fail('duplicate final acceptance marker');
      if (seenTargets.length !== required.length) fail('final acceptance marker before all targets passed');
      finalSeen = true;
      continue;
    }
    // Any other F20_NATIVE_* line is a foreign/spoofed marker; a plain
    // diagnostic line (no marker prefix) is allowed as interleaved output.
    if (line.startsWith('F20_NATIVE_')) fail(`unrecognized native marker: ${line}`);
  }

  if (seenTargets.length !== required.length) {
    fail(`missing target markers: expected ${required.length}, saw ${seenTargets.length}`);
  }
  // Enforce declared order (each seen target at its required index).
  for (let i = 0; i < required.length; i++) {
    if (seenTargets[i] !== required[i]) fail(`target markers out of order at index ${i}`);
  }
  if (!finalSeen) fail('missing final platform acceptance marker');
  return { platform, targets: seenTargets, commit, tree, nonce };
}

// High-level convenience: verify a native-log at a path using the no-follow
// exact-byte reader, so callers cannot accidentally reopen the pathname.
export function verifyNativeLogFile(path, expected) {
  const bytes = readExactBytesNoFollow(path);
  return verifyNativeLog(bytes, expected);
}

// ---- request writer subcommand ---------------------------------------------
// `node f20-native-uat-proof.mjs request '<json-request-tuple>'` durably
// persists the pending native-proof request into a Git-private authority path
// (resolved via `git rev-parse --git-path`, never a worktree-tracked file), so
// terminal-plan preparation writes exactly the object verification later
// authenticates. Exact-tuple idempotent; malformed input or a conflicting
// existing request fails closed. Performs NO push and NO dispatch.

// Resolve the Git-private authority path for a candidate's pending request.
// `git rev-parse --git-path` yields a path under the private git dir (e.g.
// .git/f20-native-uat/<commit>/request.json), which is never committed.
function gitPrivateRequestPath(commit) {
  const out = execFileSync('git', ['rev-parse', '--git-path', `f20-native-uat/${commit}/request.json`], {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  return out.trim();
}

function runRequestSubcommand(args) {
  const json = args[0];
  if (typeof json !== 'string' || json.length === 0) {
    fail('request subcommand requires a single JSON request-tuple argument');
  }
  let requested;
  try {
    requested = JSON.parse(json);
  } catch {
    fail('request tuple argument is not valid JSON');
  }
  validateRequest(requested);
  const dest = gitPrivateRequestPath(requested.commit);
  mkdirSync(dirname(dest), { recursive: true, mode: 0o700 });
  const persisted = persistRequest(dest, requested);
  process.stdout.write(serializeRequest(persisted));
}

// Run the CLI only on direct execution, never on import — keeps the module a
// pure library for the test suite and terminal-plan composition (importing it
// has no side effect).
if (process.argv[1] && realpathSync(process.argv[1]) === realpathSync(fileURLToPath(import.meta.url))) {
  const [, , subcommand, ...rest] = process.argv;
  try {
    if (subcommand === 'request') {
      runRequestSubcommand(rest);
    } else {
      process.stderr.write(`unknown subcommand: ${subcommand ?? '(none)'}\n`);
      process.exit(2);
    }
  } catch (err) {
    process.stderr.write(`${err && err.message ? err.message : err}\n`);
    process.exit(1);
  }
}
