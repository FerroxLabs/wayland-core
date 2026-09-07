# Qualification prerequisites and shared fixtures

`scripts/qualification-prerequisites.py` reports `ADMITTED` or `BLOCKED` for one
declared prerequisite. Admission is not a test result, build identity proof, or
release approval. Every required qualification gate remains required.

## Fast admission: before builds

Run only prerequisites of the selected workload. These commands neither compile
Rust nor contact an LLM provider or the OS keyring. The helper requires Python
3.11 or newer (the standard-library TOML reader).

```sh
python3 .github/scripts/tests/qualification-prerequisites.test.py
python3 scripts/qualification-prerequisites.py artifact --on-path python3
python3 scripts/qualification-prerequisites.py windows-runner --event "$GITHUB_EVENT_PATH" --runners runners.json
```

For the Windows route, generate `runners.json` immediately before this check with
`gh api "repos/$GITHUB_REPOSITORY/actions/runners" > runners.json`, using the
existing authenticated environment. Never pass a token on the command line.
The existing `windows-hosted` PR label and fork-PR condition select
`windows-latest`; those routes do not need `--runners`. Otherwise the snapshot
must contain an online runner with `self-hosted,Windows,X64,msvc` labels.
An online runner may be busy; this check does not promise queue completion.
Apply a route-changing label before starting the next workflow run. It cannot
reschedule an already-queued job.

Only container-execution workloads require this additional check:

```sh
python3 scripts/qualification-prerequisites.py linux-container-engine
```

It asks the actual daemon for `docker info --format '{{json .}}'` and rejects
Windows-container engines. It pulls no images and starts no containers. Do not
invoke it for file/text, ACP, provider, or other non-container workloads.

## Affected checks: use the matching build

Keep package, feature, target, profile, and filter selections identical between
listing and execution. Listing may build the selected targets; reuse that target
directory under the separate source/toolchain/build-identity gate.
Collection requires an explicit package and `--lib`, `--test NAME`, or
`--bin NAME`. A nextest `-E` expression only filters tests; by itself it still
builds the workspace. Missing or workspace-wide build selection is refused.

```sh
python3 scripts/qualification-prerequisites.py inventory --collect --input selection.json --require required_case -- -p package_name --test target_name -E 'test(required_case)'
python3 scripts/qualification-prerequisites.py inventory --input selection.json --require required_case
```

The collector runs `vx cargo --version` before opening the JSON capture, then
`vx --no-auto-install cargo nextest list … --message-format json`. Cold installer
output therefore cannot become inventory data. Missing/malformed inventories,
unknown testcase shapes, empty runnable selections, and missing required names
fail. Named phase markers precede initialization and listing; raw command stderr
is inherited by the caller's diagnostic capture. `--include-ignored` is appropriate only when the actual run explicitly
includes ignored cases. Run nextest with `--no-tests=fail` as a second check;
inventory admission does not establish that a test executed.

After the separate build-identity gate has admitted the evaluator artifact:

```sh
python3 scripts/qualification-prerequisites.py artifact --file "$EVALUATOR" --executable
python3 scripts/qualification-prerequisites.py scenario --evaluator "$EVALUATOR" --paired-task task.json
# Or select catalog scenarios with repeated --scenario arguments.
```

This delegates to the actual evaluator's `--dry --strict` path with an explicit
OpenAI override, a fake child-only key, and a loopback URL. It requires positive
runnable count and zero skips. That path resolves the real scenario/platform
contract before candidate execution; the Python helper maintains no family map.
This is platform admission, not macOS/Windows execution evidence.

## Shared fixture preconditions

- **Durable CLI/ACP subprocesses:** use
  `crates/wcore-cli/tests/support/vault.rs::configure_process`; own the child with
  `support/owned_tree.rs::OwnedTree`. Give the child a private `WAYLAND_HOME` and
  explicit `CredentialsBackend::EncryptedFile` paths inside its temporary root.
  Preserve `session.enabled = true` and `require_durability = true`. Do not use
  the developer's keyring or mutate the parallel parent process environment.
- **Direct engine tests:** reuse `AgentEngine::use_recovery_test_key` and
  `crates/wcore-agent/tests/common/mod.rs::RECOVERY_TEST_KEY` when testing an
  unrelated engine behavior. Dedicated encrypted-vault/recovery tests still
  exercise real sealing and reopen behavior.
- **Executable fixtures:** resolve their required commands from the fixture's
  actual PATH (`artifact --on-path python3`, for example). A known executable
  must be present before starting the scenario; do not rely on an interactive
  user's PATH being inherited by a service account.
- **Tests whose subject is process-global auth state:** reuse the existing
  `LadderEnv::scoped` with the same `serial(auth_credentials_env)` guard used by
  its callers in `wcore-cli/src/auth.rs`. Do not introduce a second guard name
  that fails to serialize existing readers. When this surface changes, run the
  existing `scripts/check-test-env-globals.py` gate; this guide does not migrate
  unrelated fixtures or replace that gate.

For a materialized subprocess fixture config, validate shape and isolation in
the child environment before qualification:

```sh
python3 scripts/qualification-prerequisites.py credentials --root "$FIXTURE_ROOT" --config "$FIXTURE_CONFIG"
```

This checks private paths, the explicit encrypted backend, strict durability,
and presence of unlock material. An inherited FD is checked with `fstat` without
reading its contents. No credential content is printed; no store is opened.
This is a precondition check, not proof that the key can seal or reopen data.

## Release qualification and experiments

After fast admission and affected checks, run the frozen release qualification
against the same admitted build identity. Retain full first-failure diagnostics.
Reuse an unaffected receipt only through the source/build compatibility gate.
Record native-platform proof separately from static admission and Linux proof.

Repeated stress experiments belong in the existing scheduled/manual workflow
paths unless the current release contract explicitly requires them. Moving an
experiment never removes a required release workload, changes a threshold, or
turns a current failure into a pass. The gate owner must name what remains
mandatory before changing scheduling.
