# Wayland Core F03 Evidence Receipt

**Status:** versioned evidence and report contract implemented and verified

**Implementation source:** `1c644ccdee8180bd2eded312d391f486be99902d` on `frontier/m0`

**Scope:** F03 only. This receipt proves the evidence schema, trust checks, redacted
projections, local publication path, and critical-usability gating. It does not
claim CI authority for a local run or invent measurements that F04/F05 have not
yet instrumented.

## 1. Delivered contract

The evaluator now provides:

- a canonical `wayland.eval.receipt` version 1 body with typed run identity,
  target, policy, lifecycle, provider, tool, decision, boundary, process,
  recovery, canary, assertion, quarantine, required-cell, result, and summary
  evidence;
- explicit `Observed` and `Unavailable` measurement states, so absent attempts,
  retries, tokens, cache, egress, filesystem, and resource evidence cannot be
  represented by plausible zero values;
- SHA-256 content addressing over the canonical body and detached,
  domain-separated Ed25519 signatures;
- an external verification policy binding the receipt to trusted key, source
  commit, binary digest, repository, ref, workflow, and authority;
- separate integrity, authority, and release-gate decisions: local receipts are
  always non-authoritative, a valid signature may attest a failed run, and a CI
  authority claim cannot satisfy the gate without external trust and complete
  observed evidence;
- recursive duplicate-key rejection plus rejection of trailing documents,
  truncated JSON, unsupported major schemas, noncanonical hashes, duplicate or
  missing cells, hollow evidence, and inconsistent totals/outcomes;
- redacted JSON, JSONL, JUnit XML, console, and Markdown projections derived
  from one receipt body digest, with stable failure identities and no raw
  prompt, model output, tool payload, stderr, call ID, secret, or worktree path;
- render-all-then-scan publication: a provider/canary secret in any projection
  rejects the complete bundle before persistence;
- per-cell atomic local publication through `wayland-eval --report-dir`, with
  no partially published bundle and no overwrite of an existing destination;
  and
- critical usability findings promoted from advisory output to failed receipt
  results even when the underlying scenario superficially passed.

## 2. Verification evidence

Verification ran on the isolated Hetzner worktree
`/root/wayland-frontier-m0` at the exact implementation source above, using
`rustc 1.95.0 (59807616e 2026-04-14)`:

- `WCORE_EVAL_REQUIRE_CONTAINMENT=1 cargo +1.95.0 test -p wcore-eval-scenarios --all-targets`:
  159 passed, 0 failed, 4 explicitly ignored live tests across 14 test binaries;
- the receipt contract covers canonical golden projections, additive v1
  compatibility, unknown-major rejection, mutation/corruption, duplicate and
  trailing JSON, incomplete evidence, explicit-unavailable gating, local
  authority, unsigned and mismatched CI authority, external trust anchors,
  projection secret scanning, and critical usability failure;
- the driver integration test executes the fixture through the exact artifact
  and JSON stream, then publishes one redacted five-file receipt bundle;
- runner contracts verify hermetic secret redaction, process cleanup, bounded
  output, deadlines, cost evidence, lifecycle timing, and correlated tool-call
  duration evidence;
- `cargo +1.95.0 clippy -p wcore-eval-scenarios --all-targets -- -D warnings`
  passed with no diagnostics; and
- `cargo +1.95.0 fmt --all -- --check` passed.

## 3. Trust and operator behavior

The standalone CLI intentionally emits local, non-authoritative receipts and
keeps ordinary pass/fail behavior usable. It exposes no signing-key flag. A
trusted CI controller must sign outside the candidate worker and the verifier
must receive its trusted public key and provenance policy out of band.

A complete local receipt may pass its scenario gate but remains unsuitable as
release authority. Conversely, a trusted signed receipt can authentically prove
failure. Release tooling must require both `AuthoritativeCi` and a passed gate;
neither status implies the other.

## 4. Honest boundary and next work

F03 records currently unmeasured provider attempts/retries/tokens/cache,
egress/filesystem deltas, and resource peaks as `Unavailable`. Those fields
therefore fail the milestone evidence gate instead of becoming fake success.
F04 must supply deterministic real-loop fixtures and measurement sources. F05
must integrate the required CI signer/trust policy and capability activation
evidence. F28/F29 remain responsible for native cross-platform certification
and the release supply-chain trust root.

This receipt does not claim that the local CLI output is authoritative, that a
self-carried signing key is trusted, or that an unavailable measurement passed.
