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

The worked example below writes a concrete manifest for a deliberately small,
helper-only contract. A real qualification manifest must instead enumerate the
already-agreed build, affected, native, and release-admission checks. Do not infer
that complete set from changed paths or remove a required check because no receipt
is available yet.

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

## Operator recipes

From the repository root, with Python 3.11+ and the pinned `just` available:

```sh
vx just qualification-plan BASE_SHA manifest.json metadata.json
vx just qualification-status manifest.json state.json receipts.json
```

The defaults are `target/qualification/plan.json`,
`target/qualification/readiness.json`, and `target/qualification/readiness.md`.
Optional trailing arguments override these paths; `qualification-status` accepts
one final phase argument, defaulting to `verified`. Both recipes create output
parents and propagate refusal/nonzero exit codes. They pass parameters as Python
argv, without interpolating path contents into a shell. Existing push, lint, and
test recipes are unchanged.

## Worked example: helper-only checks at an exact source

This runnable example qualifies **only this helper's syntax and focused tests**.
It neither builds Core nor qualifies a release. Its manifest intentionally has no
reviewed reusable input closure, so it cannot reuse results from another SHA.
The full release/native qualification remains mandatory afterward, using the
frozen release manifest and authenticated CI/native receipts.

Start from a clean committed checkout. Generate inputs outside tracked source;
otherwise embedding the candidate SHA in a tracked manifest would change that SHA.
The example uses the parent commit only to demonstrate planning, not as a claimed
verified baseline. Metadata inspection below does not compile Rust. Perform real
Rust build/test recipes only on the approved native executor.

```sh
export QUAL_INPUTS="$(mktemp -d)"
python3 - <<'PYCODE'
import json, os, pathlib, platform, subprocess, sys, sysconfig
repo = pathlib.Path(subprocess.check_output(
    ['git', 'rev-parse', '--show-toplevel'], text=True).strip())
out = pathlib.Path(os.environ['QUAL_INPUTS'])
def git(*args):
    return subprocess.check_output(['git', '-C', str(repo), *args], text=True).strip()
source = git('rev-parse', 'HEAD')
assert not git('status', '--porcelain'), 'commit or isolate changes first'
# Warm vx outside JSON capture; metadata reads the workspace without compiling.
subprocess.run(['vx', 'cargo', '--version'], cwd=repo, check=True)
raw = subprocess.check_output(['vx', '--no-auto-install', 'cargo', 'metadata',
    '--no-deps', '--format-version', '1', '--locked'], cwd=repo, text=True)
assert git('rev-parse', 'HEAD') == source and not git('status', '--porcelain')
(out / 'metadata.json').write_text(json.dumps({'source_sha': source, 'cargo': json.loads(raw)}))
identity = {'toolchain': {'python': sys.version}, 'features': [],
    'target': sysconfig.get_platform(),
    'runtime_contract': {'executor': platform.node(), 'scope': 'helper-only',
                         'python': sys.executable},
    'owner': 'core', 'inputs_complete': False, 'inputs': []}
checks = [
    dict(identity, id='helper-syntax', stage='build', kind='build',
         command=[sys.executable, '-m', 'py_compile', 'scripts/qualification-status.py']),
    dict(identity, id='helper-self-tests', stage='affected', kind='gate',
         command=[sys.executable, '.github/scripts/tests/qualification-status.test.py'])]
manifest = {'schema': 1, 'candidate_sha': source, 'pr_head_sha': None,
            'release_tag': None, 'checks': checks, 'fixture_owners': {}}
(out / 'manifest.json').write_text(json.dumps(manifest, indent=2))
(out / 'state.json').write_text(json.dumps(
    {'checkout_sha': source, 'pr_head_sha': None, 'worktree_clean': True}, indent=2))
(out / 'receipts.json').write_text('[]')
(out / 'base.sha').write_text(git('rev-parse', 'HEAD^'))
PYCODE
vx just qualification-plan "$(cat "$QUAL_INPUTS/base.sha")" "$QUAL_INPUTS/manifest.json" "$QUAL_INPUTS/metadata.json"
```

Review `target/qualification/plan.json` and the explicit manifest commands before
execution. The following executor runs **only** `commands[].argv`, never joins them
into a shell string, and preserves stdout/stderr and measured command results. It
requires `full` for this example because no verified baseline was supplied. A real
`affected` plan additionally needs the verified baseline and individually admitted
reuse evidence; do not delete that prerequisite to make the example run.

```sh
python3 - <<'PYCODE'
import json, os, pathlib, subprocess
repo = pathlib.Path.cwd()
out = pathlib.Path(os.environ['QUAL_INPUTS'])
manifest = json.loads((out / 'manifest.json').read_text())
plan = json.loads((repo / 'target/qualification/plan.json').read_text())
source = manifest['candidate_sha']
assert plan['candidate_sha'] == source and plan['disposition'] == 'full'
checks = {row['id']: row for row in manifest['checks']}
assert plan['required_checks'] == list(checks)
receipts = []
env = dict(os.environ, PYTHONPYCACHEPREFIX=str(out / 'pycache'))
def unchanged():
    assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip() == source
    assert not subprocess.check_output(['git', 'status', '--porcelain'], text=True).strip()
for entry in plan['commands']:
    unchanged()
    check = checks[entry['id']]
    assert entry['argv'] == check['command'], 'plan/manifest command mismatch'
    with (out / (entry['id'] + '.stdout')).open('wb') as stdout, \
         (out / (entry['id'] + '.stderr')).open('wb') as stderr:
        result = subprocess.run(entry['argv'], cwd=repo, env=env,
                                stdout=stdout, stderr=stderr, shell=False)
    unchanged()
    receipt = {key: check[key] for key in
        ('id', 'command', 'toolchain', 'features', 'target', 'runtime_contract')}
    receipt.update(source_sha=source, complete=True, exit_code=result.returncode,
                   status='passed' if result.returncode == 0 else 'failed',
                   attempts=1, flaky=False)
    receipts.append(receipt)
    (out / 'receipts.json').write_text(json.dumps(receipts, indent=2))
    if result.returncode != 0:
        raise SystemExit(result.returncode)
PYCODE
vx just qualification-status "$QUAL_INPUTS/manifest.json" "$QUAL_INPUTS/state.json" "$QUAL_INPUTS/receipts.json"
```

The resulting verification applies only to the two named helper checks. Merge and
publication remain pending, and no Core release readiness is established. For a
PR, collect its latest actual head separately instead of using this non-PR
example's `null`; do not substitute the checkout SHA for that observation.
For a real `kind: test` receipt, derive selected/passed/failed/skipped/retry counts
from its captured test evidence; never copy the helper example's exit-only gate
shape. Retain original source IDs on any reused passes and run the remaining
frozen release qualification before treating the release manifest as verified.
