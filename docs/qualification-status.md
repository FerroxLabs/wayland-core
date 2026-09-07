# Affected planning and qualification status

`scripts/qualification-status.py` consumes a required-check manifest, Git objects,
Cargo metadata, and authenticated evidence collected by the caller. It executes
no checks, contacts no service, and publishes nothing. It supplements the
existing release gates; it does not replace their required workloads or thresholds.

```sh
python3 .github/scripts/tests/qualification-status.test.py
python3 scripts/qualification-status.py plan --repo . --base BASE_SHA --manifest qualification.json --cargo-metadata metadata.json --output plan.json
python3 scripts/qualification-status.py status --repo . --manifest qualification.json --state state.json --receipts receipts.json --output readiness.json --markdown readiness.md
```

The `plan` output has `disposition: affected|full`, reason records, exact
`commands[].argv`, and each affected target's Cargo package/binary selectors.
Only direct integration-test source changes or explicitly single-owner fixtures
can select `affected`. Owner IDs must resolve to real candidate Cargo test targets
and a declared affected check. Production, manifests/configuration, shared helpers,
ambiguous ownership, and unknown paths select `full`. Invalid metadata refuses the
plan. `unchanged` is recorded separately in `mode` without inventing a rerun.

An affected plan is not qualification and is not permission to skip full checks.
Before using it for an affected-only rerun, the caller must have a verified
baseline and separately admit every reused receipt. `requires_verified_baseline`
is always true. Without that proof, use full qualification. `required_checks`
always retains the complete declared list, even when `next_checks` is smaller.

## Manifest and metadata

Use full 40-character Git SHAs. The candidate is the actual checkout/build source;
the expected PR head is separate because a PR merge checkout can have a different
SHA. Freeze the manifest with the agreed checks; the helper does not discover or
silently reduce release requirements.

Minimal illustrative manifest (replace the fixture commands/contracts with the
actual required build, affected, native, and release-gate checks):

```json
{
  "schema": 1,
  "candidate_sha": "FULL_CHECKOUT_SHA",
  "pr_head_sha": "FULL_PR_HEAD_SHA_OR_NULL",
  "release_tag": "vX.Y.Z",
  "checks": [
    {
      "id": "core-build",
      "stage": "build",
      "kind": "build",
      "owner": "core",
      "command": ["cargo", "build", "-p", "wcore-cli", "--bin", "wayland-core"],
      "toolchain": {"rustc": "EXACT_VERSION", "linker": "EXACT_IDENTITY"},
      "features": [],
      "target": "x86_64-unknown-linux-gnu",
      "runtime_contract": {"profile": "debug", "executor": "DECLARED_ENVIRONMENT"},
      "inputs_complete": false,
      "inputs": []
    },
    {
      "id": "paired-contract",
      "stage": "affected",
      "kind": "test",
      "owner": "core",
      "command": ["cargo", "nextest", "run", "-p", "wcore-eval-scenarios", "--test", "paired_fixture_contract", "--retries", "0", "--no-tests=fail"],
      "toolchain": {"rustc": "EXACT_VERSION", "linker": "EXACT_IDENTITY"},
      "features": [],
      "target": "x86_64-unknown-linux-gnu",
      "runtime_contract": {"profile": "debug", "executor": "DECLARED_ENVIRONMENT"},
      "inputs_complete": false,
      "inputs": [],
      "cargo_targets": ["wcore-eval-scenarios::test::paired_fixture_contract"]
    }
  ],
  "fixture_owners": {}
}
```

Stages are `fast`, `build`, `affected`, or `release`; kinds are `gate`, `build`,
or `test`. Include existing release-admission and outstanding native requirements
as checks, not only the two illustrative rows. For non-PR work, `pr_head_sha` is
JSON `null`, not a string. The metadata file is
`{"source_sha":"FULL_CANDIDATE_SHA","cargo":{...}}`, where `cargo` is the real
candidate's `cargo metadata --no-deps --format-version 1` output. Cargo metadata
collection must itself be bound to the selected source, not relabeled afterward.

An external fixture owner entry is a repository-relative path mapped to a list
of target IDs, such as `{"crates/pkg/tests/fixtures/input.json":["pkg::test::case"]}`.
Multiple owners require full qualification. Do not label a shared helper as a
single-owner fixture to narrow the plan.

## Receipts and reuse

`receipts.json` is an array with exactly one selected receipt per required ID.
Each receipt retains its original `source_sha` and has `id`, `complete: true`,
`status: "passed"`, integer `exit_code: 0`, integer `attempts: 1`, `flaky: false`,
and the exact manifest `command`, `toolchain`, `features`, `target`, and
`runtime_contract`. Test receipts additionally require integer
`selected == passed > 0`, `failed == skipped == retries == 0`.
Here `skipped` means selected cases that did not run; filtered-out cases can be
recorded separately and never count as selected/passed. Missing, pending, skipped,
failed, flaky, retried, duplicate, or identity-mismatched receipts cannot verify.

Old-source reuse additionally requires an explicitly reviewed complete input
closure: `inputs_complete: true`, a nonempty exact `inputs` list, and
`input_sha256: {"repo/path":"SHA256_OF_FILE_BYTES"}` on the receipt. Every declared
input must be a regular tracked file, with the same bytes in both original and
candidate Git objects. Receipt hashes must equal that complete map. Missing files,
symlinks, missing closure entries, or a command/toolchain/feature/target/runtime
change refuse reuse. No dependencies are guessed. Start with
`inputs_complete: false` until the owner has actually established the closure;
this admits only matching-source receipts. A complete closure also validates
hashes for matching-source receipts.

The JSON and Markdown output preserve `receipt_source_sha` and `mode: reused`.
They never rewrite old receipts to claim current-source execution. The collector
must authenticate the underlying run, output artifacts, and source binding;
arbitrary user-authored JSON is not evidence merely because it has these fields.

## Readiness and publication facts

`state.json` contains the observed `checkout_sha`, boolean `worktree_clean`, and
latest observed `pr_head_sha`. A mismatch blocks candidate readiness. Optional
`merge` is an actual PR record with `merged`, `merged_at`, `head_sha`, and
`merge_commit_sha`. Optional `release` is an actual release record with `id`,
`draft`, `published_at`, `html_url`, `tag_name`, and independently resolved
`resolved_source_sha`. Publication requires a non-draft published release whose
tag matches the manifest and whose resolved source equals the candidate. A tag
name, tag existence, or unresolved `target_commitish` alone is insufficient.

The dashboard reports committed, built, verified, merged, and published separately.
`ready` requires fresh identity, nonempty build evidence, an affected/release check,
and every required check passing. `complete` additionally requires actual merge
and publication records. An already-published release can be shown alongside
failed qualification without making `ready` true. Default exit status gates
`verified`; `--require-phase committed|built` supports explicit earlier stage
checks. `--require-phase merged|published` still requires full verification.
Malformed input exits nonzero and replaces any old green output with a refused
report. None of these commands merges, tags, or publishes.
