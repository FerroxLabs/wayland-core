# Evaluation evidence receipts

`wayland-eval` emits schema-versioned, content-addressed evidence for each
scenario/provider/platform cell. The canonical artifact is `receipt.json`;
JSONL, JUnit XML, console text, and Markdown are projections of that receipt and
carry the same body digest.

## Local use

```bash
wayland-eval \
  --scenario canary \
  --provider deepseek \
  --binary ./wayland-core \
  --expected-source-commit "$SOURCE_COMMIT" \
  --report-dir ./eval-reports
```

Each cell is published as one atomic directory containing:

- `receipt.json` — canonical versioned envelope;
- `events.jsonl` — header, one record per cell, and trailer;
- `junit.xml` — CI test projection;
- `report.txt` — compact console projection;
- `report.md` — review projection.

Rendering scans every complete projection for the provider credential before
any file is written. The receipt contains hashes and stable failure codes, not
raw prompts, model output, stderr, host paths, tool arguments/results, call IDs,
or failure details.

Local receipts are always `local_non_authoritative`. This is intentional: a
developer can run useful diagnostics without CI credentials, but cannot turn a
local result into release proof.

## Trust and authority

Authority is derived, not asserted. A CI receipt is authoritative only when all
of these checks pass in `ReceiptVerifier`:

1. schema name and major version are supported;
2. JSON contains no duplicate keys, trailing document, or truncation;
3. every required evidence group is present and structurally valid;
4. the canonical body digest matches `body_sha256`;
5. the detached Ed25519 signature verifies under a key trusted out of band;
6. source commit, executed binary digest, repository, source ref, and workflow
   match the verifier's external policy;
7. the required-cell manifest exactly matches the unique result cells;
8. result verdicts and summary totals recompute from the evidence.

A public key included by the producer is not a trust anchor. The verifier must
be configured independently. Signing credentials must live in the trusted CI
packaging/attestation step, never in the disposable worker that executes the
candidate agent. `wayland-eval` itself emits local receipts and does not accept
a signing-key flag.

The signature covers a domain-separated digest of the complete redacted body.
Changing source, binary, config, fixture, policy, result, or any nested evidence
invalidates the digest and signature.

## Integrity versus gate status

Receipt integrity, provenance authority, and release outcome are separate:

- A local receipt can have valid integrity but is never authoritative.
- An authoritative receipt can truthfully attest a failed run.
- A green local cell remains convenient even if optional release-grade
  instrumentation is unavailable.
- A milestone gate requires authoritative CI provenance, a passing result, and
  complete security/accounting evidence.

Evidence that was not measured is encoded as
`{"state":"unavailable","code":"..."}`. It is never replaced with an empty
list or zero. Provider attempts/retries, token/cache accounting, egress,
filesystem deltas, peak resources, orphan status, and other release evidence
must be observed before the milestone gate can pass.

High-severity usability findings such as a background panic or broken subsystem
turn a superficially successful scenario into a failed receipt cell. The stable
finding code is retained while the raw evidence line is hashed.

## Compatibility

Schema v1 readers accept additive fields, but reject an unknown major version.
Unknown additive fields do not change v1 semantics. Duplicate JSON keys are
rejected recursively so different parsers cannot interpret the same receipt in
different ways.

All monetary fields in the receipt use integer microdollars. Durations use
integer milliseconds. This avoids floating-point drift in content addressing
and makes golden report bytes stable across supported platforms.
