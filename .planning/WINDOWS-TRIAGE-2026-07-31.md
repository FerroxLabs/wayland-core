# WINDOWS TRIAGE — 2026-07-31

Lane: `win-triage` (`lane/win-triage`). Nothing here is pushed, merged, PR'd or closed.

---

## 0. The headline, before anything else

**Windows CI on `plan/f20-unified-audit-repair` has run ZERO tests for seven
consecutive runs, and it is STILL running zero tests at the branch tip.**

The brief said "clippy is now fixed". It is not fixed on Windows. I checked every
`CI (Array)` job (that is the self-hosted Windows leg — the job name renders as
`Array` because `runs-on` is a label list) in the last 60 CI runs across all
branches. **Not one of them reached the test step.** Every single one:

```
success  Check formatting
skipped  No vacuous `cargo test` invocations
failure  Clippy (warnings = errors)          <-- dies here
skipped  Run tests (nextest CI profile)      <-- never runs
skipped  Check Desktop protocol contract corpus drift
skipped  Release binary smoke
skipped  F01 packaged wayland-eval driver gate
skipped  Eval acceptance gate
skipped  Security audit
```

Verified at the branch tip `4fadbd4d` (run `30629949130`, job `91165565837`) and at
the base `659fa492` (run `30609992296`, job `91090433391`) — identical. Also red on
`lane/ci-clippy` itself (run `30613173832`, job `91100347490`).

The `ci-clippy` lane closed a *different* pair of clippy errors. The one that is
still killing Windows is in a file that lane did not touch. It is fixed here (§3.1).

**So: the "~40 failures" in the brief did not come from CI.** There is no CI run on
this branch that produced a Windows test number. I got one by running the suite
myself on the real box, and I also found the one historical CI run that did
enumerate Windows. Both numbers are below.

---

## 1. The two real Windows test numbers

| | run A — CI, service account | run B — mine, real box |
|---|---|---|
| where | `CI (Array)`, run `30403867920`, job `90424728470` | `ssh SeanD@seandesktop`, `D:\wintriage\repo` |
| commit | `189599ca` (2026-07-28) | `62b048a5` (lane base = `4fadbd4d` + 2 macOS fixes) |
| account | runner service account (`C:\WINDOWS\SERVIC~1\NETWOR~1\…`) | interactive user `seand` |
| command | `vx just test-ci` | `vx cargo nextest run --workspace --profile ci --no-fail-fast` |
| **result** | **12469 run, 12388 passed, 81 failed, 116 skipped** | **13347 run, 13307 passed, 40 failed, 130 skipped** |

Run B is where the brief's "~40" comes from. It reproduces.

**Neither number was reached by CI at the current tip.** Run A is a week old and
predates the merge; run B is mine and is not a CI verdict.

### The differential is the most useful thing in this document

26 tests fail in **both** environments — those are durable, code-level Windows
defects. 55 fail only under the service account, and 14 only under mine. That split
is what separates code from environment, and I used it as the primary classifier
rather than guessing.

---

## 2. Root-cause table

Counts are from **run B (40)** unless the row says otherwise. "Pre-existing" is
against base `659fa492`; every row is pre-existing — the merge introduced none of it.

| # | Root cause | Count | Class | Where it lives | Pre-existing? |
|---|---|---|---|---|---|
| **W0** | **`#[cfg(unix)]` test with un-gated helpers ⇒ dead code ⇒ `-D warnings` kills `just lint` before the test step** | blocks **all** | **(a) real defect, CI-fatal** | `crates/wcore-browser/tests/downloads_root_baseline_test.rs:47,77,127,147` | yes — identical at `659fa492` | 
| **W1** | `/`-rooted literals fail `is_absolute()` on Windows (root, but no drive `Prefix`) | 8 | (b) test-only | `wcore-eval-scenarios/tests/receipt_contract.rs:115,371,373,389,393,410,467,471,475,914,990`; `openai_fixture_contract.rs:81`; `wcore-agent/src/child_transaction/gate_executor.rs:652`; `wcore-agent/src/journal_effects.rs:1565`; `wcore-tools/src/transcription_tools.rs:833` | yes |
| **W2** | test `unwrap()`s an API that is explicitly `Unsupported` on Windows — *"identity-aware file observation is unavailable on this platform"* | 2 | (b) test-only | `crates/wcore-agent/src/engine.rs:26649` | yes |
| **W3** | code writes a restrictive DACL then cannot reopen its own file — `Os { code: 5, PermissionDenied }` | 4 | **(a) real defect** | `wcore-agent/src/session_journal/snapshot.rs:1455,1468`; `wcore-cli/src/log_rotate.rs:250,358` | yes |
| **W4** | AppContainer sandbox child execution fails — *"sandbox child execution failed"* | 7 | **(a) real defect** | see §2.1 — strongly indicated: `wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs:211,302` | yes |
| **W5** | owned descendants are not reaped on Windows — `Failure::Hung { stderr_tail: "" }` | 5 | **(a) real defect** | `wcore-eval-scenarios/tests/runner_contracts.rs:580,647,672,693`; reaper in `wcore-eval-scenarios/src/process_tree.rs` | yes |
| **W6** | swarm worktree/transaction roots are never reclaimed on Windows (delete-with-open-handle) | 7 | **(a) real defect** | `wcore-swarm/src/worktree_tests.rs:193,260`; `tests/dispatch_smoke.rs:33,94,369`; `tests/worker_runtime_limits.rs:39` | yes |
| **W7** | transport error classified `Timeout` where the contract says `Connect` | 1 | (a) real defect, low | `crates/wcore-egress/src/lib.rs:563` | yes |
| **W8** | voice-mode player argv — *"The filename, directory name, or volume label syntax is incorrect"* | 1 | (a) real defect | `wcore-agent/src/tool_backends/voice_mode.rs:679,1071,1096` | yes |
| **W9** | `--json-stream` emits no `ready` within 10s | 1 (run B) / 6+ (run A) | **(a) real defect, HIGH** | `wcore-cli/tests/harness_regression.rs:1259`; engine startup path | yes |
| **W10** | path-form/casing comparison of a resolved executable | 1 | (a) real defect, low | `wcore-config/src/shell/executable_readiness_tests.rs:721` | yes |
| **W11** | `try_canonicalize("/nonexistent/path/xyz")` returns `Some` | 1 | **(c) infrastructure — box contamination** | `wcore-skills/src/loader_tests.rs:54` | n/a |
| **W12** | `packaged_core_exhausts_a_real_read_timeout` — attempt count 0 vs 3 | 1 | unclassified | `wcore-cli/tests/deterministic_openai_loop.rs:218` | yes |
| **W13** | hard-coded `python3` (**run A only**, absent from run B) | 23 | (b) test-only + (c) infra | `wcore-cli/tests/portability_hostile_corpus.rs:119,433,520,533,1110` | yes |
| **W14** | *"Session persistence authority unavailable: secure recovery storage is unavailable"* (**run A only**) | ~20 | **(c) infrastructure — runner account** | credential vault / DPAPI unavailable to the service account | yes |
| **W15** | `binary_matches_repo_head` compares a 40-char sha to a 7-char sha (**run A only**) | 1 | (b) test-only | `wcore-cli/tests/build_provenance.rs:99` | yes |
| **W16** | Desktop contract corpus drift (**run A only** — fixed by regeneration #7) | 1 | (b) known bookkeeping | `wcore-protocol/tests/desktop_contract_corpus.rs:203` | fixed since |

### 2.1 W4 — the mechanism I think is underneath it

A parallel static sweep found this, and it lines up with W4 exactly.
`crates/wcore-sandbox/src/backends/appcontainer/windows_impl/command.rs:211`:

```rust
pub(super) fn is_unc_or_device_path(p: &str) -> bool {
    p.starts_with("\\\\") || p.starts_with("//")
}
```

`resolve_program` (`:302`) uses this to reject argv[0], **but never consults
`is_verbatim_disk_path` (`:222`)** — whose own doc comment, eight lines above, says
`std::fs::canonicalize` returns the `\\?\` form for *every* local path on Windows and
that the guard must therefore treat it as local. So a canonicalized program path
`\\?\C:\…\sh.exe` is classified as an NTLM-relay vector and refused. The test at
`windows_impl/tests.rs:372` currently *asserts* that refusal, so it is locked in.

`strip_verbatim_disk_prefix` (`:253`) has exactly **one** call site in the entire
workspace (`process.rs:404`). This is unverified as W4's cause — I did not get a
debug repro — but it is the first thing to check.

### 2.2 Why W11 is the box and not the code

`try_canonicalize` is `std::fs::canonicalize(path).ok()`. On Windows
`/nonexistent/path/xyz` resolves against the current drive. Measured on the box:

```
D:\nonexistent exists: True
D:\nonexistent\path\xyz exists: True
```

A previous lane's run left that tree on `D:`. The test is correct; the host is dirty.
It does not appear in run A (different account, different drive). This is the kind of
thing that would otherwise have been filed as a Windows canonicalisation bug.

---

## 3. Hardware vs code — the Raptor Lake question, answered

**None of the 40 is hardware.** The `ci` nextest profile sets `retries = 2`, so every
failing test runs three times. **All 40 failed all three attempts.** An intermittent
Raptor Lake execution fault does not reproduce deterministically 3-for-3, 40 times.
The run also reported `2 flaky` — two tests that failed once and passed on retry.
Those are the population where a hardware story is even admissible, and they are not
in the failure set.

**The rustc access violation is unsubstantiated by any log I can reach.** I swept
every `CI (Array)` and `Build (*-pc-windows-msvc)` job in the last 25 CI runs for
`access violation`, `0xc0000005` and `rustc-ice`: **zero matches**. The
`Pre-build wcore-cli release binary` step is recorded `success` in all seven Windows
jobs on this branch. My own build of the full workspace on that box completed in
5m20s with no crash. I am not saying it never happened — I am saying it is not in the
evidence, so I will not carry it forward as a finding. The box is a 13900KF; the risk
is real; this particular claim is not currently supported.

---

## 4. What I fixed, with ablation

Commit `0ee602b6`. Verified on the real box at that commit.

### 4.1 W0 — the CI blocker

`crates/wcore-browser/tests/downloads_root_baseline_test.rs`. The file's only
symlink-dependent test is `#[cfg(unix)]` and its author knew it — the doc comment says
the companion test is deliberately un-gated "so this file never runs ZERO tests on
Windows". But the three helpers it alone uses, plus the `PathBuf` import, were not
gated, so on Windows they are dead code and `-D warnings` fails `just lint`.

**Ablation (red):** the un-gated code, on Windows, run `30629949130`:
```
error: unused import: `PathBuf`                    --> ...:47:23
error: method `download_dests` is never used       --> ...:77:8
error: function `tool_with_root` is never used     --> ...:127:4
error: function `attempt` is never used            --> ...:147:10
error: could not compile `wcore-browser` (test "downloads_root_baseline_test") due to 4 previous errors
error: Recipe `lint` failed on line 88 with exit code 1
```
**After (green), measured on the box at `0ee602b6`:**
```
vx cargo clippy -p wcore-browser --all-targets -- -D warnings
===== CLIPPY_EXIT=0 =====
```
macOS `cargo clippy -p wcore-browser --all-targets -- -D warnings` → exit 0 (the unix
arm is untouched). `cargo fmt --all` clean.

This is the fix that matters. Without it the other seven steps of the Windows job stay
skipped, and `skipped` keeps reading as `passed`.

### 4.2 W1 (partial) — 5 of the 8

`wcore-eval-scenarios/tests/receipt_contract.rs` and `openai_fixture_contract.rs`.
`Path::new("/private/ephemeral").is_absolute()` is **false** on Windows, and
`workspace_forms` (`crates/wcore-eval-scenarios/src/workspace_evidence.rs:29`) opens
with `!workspace.is_absolute()`. Five tests died on
`"workspace must be an absolute non-root path"` before measuring anything. The roots
are synthetic identity fixtures — never created, never opened — so a `C:` prefix on
Windows preserves the semantics exactly.

**Ablation (red):** all five in the run-B failure list at `62b048a5`.
**After (green), on the box at `0ee602b6`:** all five `PASS`.

### 4.3 W13 — `python3`

Five `Command::new("python3")` sites in
`crates/wcore-cli/tests/portability_hostile_corpus.rs`. A python.org/winget Windows
install ships `python.exe` and no `python3.exe`; the only `python3` on a stock box is
the per-user Microsoft Store alias under `%LOCALAPPDATA%\Microsoft\WindowsApps`, which
the runner's service account does not have. Now uses the convention already in the
tree at `wcore-agent/src/orchestration/anvil/detect.rs:59`.

**Ablation (red):** 23 failures in run A, all `program not found`.
**After (green), on the box at `0ee602b6`:** 23/23 `PASS`, including
`hostile_conservation_invariant_balances_across_every_corpus` at 13.6s — a real
execution, not a skip.

### 4.4 The combined verification

```
D:\wintriage\repo @ 0ee602b6
vx cargo clippy -p wcore-browser --all-targets -- -D warnings   ->  CLIPPY_EXIT=0
vx cargo nextest run --profile ci --no-fail-fast \
   -p wcore-eval-scenarios --test receipt_contract --test openai_fixture_contract \
   -p wcore-cli --test portability_hostile_corpus
Summary [13.617s] 50 tests run: 50 passed, 0 skipped                ->  TESTS_EXIT=0
```
`0 skipped` is load-bearing here. This document's whole premise is that a skip is not
a pass.

---

## 5. What I did NOT fix, ranked, with honest cost

| rank | root cause | count | why it is next | cost |
|---|---|---|---|---|
| 1 | **W9** `--json-stream` emits no `ready` | 1–6+ | `ready` is the first thing Wayland Desktop consumes. Already independently measured against the runner's own `target\release\wayland-core.exe` under three environments — `<NO LINE>` in all three, so it is not env-var-shaped. Linux completes the same handshake in <0.2s. | 1–2 days; needs a debug repro on the box, not static reading |
| 2 | **W4** AppContainer child exec | 7 | Highest test count among real product defects, and there is a concrete named suspect (§2.1). But `windows_impl/tests.rs:372` asserts the current behaviour, so fixing it means arguing that test is wrong — a security-boundary decision, not a patch. | 1–2 days + a security review; **do not do this without Sean** |
| 3 | **W6** swarm worktree never reclaimed | 7 | Windows refuses to delete a directory with an open handle. Needs the handle owner found and scoped, per the pattern `session_journal/snapshot.rs:1185-1203` already documents. | 1 day |
| 4 | **W5** descendants not reaped | 5 | Windows needs a Job Object, not a process-group kill. `wcore-sandbox` already has the Job Object machinery; `wcore-eval-scenarios/src/process_tree.rs` does not use it. Note the sibling macOS EPERM problem in the same file (handoff §4B) — **do these together or the second one re-breaks the first.** | 1–2 days |
| 5 | **W3** restrictive-DACL self-lockout | 4 | The code sets a DACL it then cannot read back. Needs someone who knows whether the intended ACE should include the writer's own SID — a correctness question about the security property, not a bug. | half day + a decision |
| 6 | **W1** remaining 3 | 3 | `gate_executor.rs:652`, `journal_effects.rs:1565`, `transcription_tools.rs:833`. Same shape as what I fixed, different crates. Mechanical. | 1–2 hours |
| 7 | **W2** unsupported-platform unwrap | 2 | Decide: implement identity-aware file observation on Windows, or `#[cfg]`-skip with a loud reason. The second is 20 minutes but leaves a real capability gap unmarked. | 20 min (skip) / days (implement) |
| 8 | **W8** voice_mode argv | 1 | Ties to two independent quoting findings in the same file (`:679` PowerShell single-quote injection, `:1071` `%` expansion inside quotes). Worth doing as one pass. | 2–3 hours |
| 9 | **W7** / **W10** / **W12** / **W15** | 4 | Small, isolated, each needs its own measurement. | 1 hour each |
| 10 | **W14** vault unavailable to service account | ~20 in CI | Not code. Either give the runner a profile that has DPAPI/credential storage, or make the tests declare and skip. **A ~20-failure block that is purely runner configuration is worth fixing first for signal quality** — it is currently the largest single contributor to CI's Windows red. | infra; half day |
| 11 | **W11** box contamination | 1 | `rm -r D:\nonexistent` on seandesktop. Not a code change. | 1 minute |

---

## 6. Adjacent findings, not from the test run

Two static sweeps ran alongside the triage. Neither is a test failure; both are the
defect families the brief asked me to look for.

### 6.1 Two-way `cfg!(windows)` where the non-Windows arm assumes Linux

The brief named one confirmed instance (`wcore-exec-backend/src/orphan.rs:321-333`,
`ps -o etimes`). Others found:

1. **`crates/wcore-exec-backend/src/node/pairing.rs:102-113`** — HIGH.
   `read_hostname_file()` reads `/etc/hostname` and `/proc/sys/kernel/hostname` under
   a comment saying *"Unix hosts publish the hostname on disk"*. Darwin has neither and
   no `/proc`. Every macOS node silently reports `machine_id = "unknown-host"`, which
   still passes `validate()`, so two Macs are indistinguishable in the registry. This
   is already **proven by a test in the tree** —
   `on_darwin_local_identity_falls_back_to_a_constant_because_the_hostname_files_are_linux_only`
   at `wcore-exec-backend/tests/node_contract.rs:174-227` — and it is not in the
   handoff's 9-cause list.
2. **`crates/wcore-cli/src/backup/platform_paths.rs:44-51`** — MEDIUM.
   `#[cfg(not(windows))] MAX_TOTAL_PATH = 4096` is Linux's `PATH_MAX`. Darwin's is
   **1024**. The guard under-refuses 4× on macOS and prints the wrong limit to the
   operator, defeating the stated purpose of the check.
3. **`crates/wcore-plugin-wasm/src/runner.rs:592-602`** — LOW/MEDIUM.
   `workspace_base_dir()` hard-codes `XDG_DATA_HOME` / `~/.local/share`. On Windows
   neither var exists so it falls to the world-writable temp dir — exactly the staging
   window its own doc comment says it exists to shrink. The sibling
   `wcore-skills/src/govern.rs:534-549` solves this correctly with `dirs::data_dir()`.
4. **`wcore-cli/tests/child_attribution_corpus.rs:93` and `child_authority_corpus.rs:92`**
   — `fn platform() -> { if cfg!(windows) {"windows"} else {"linux"} }`. Both write a
   results artifact keyed by that string, so a macOS run files itself as a Linux row.
   A corpus that cannot name the platform it measured is the same failure shape as six
   runs of `skipped`.

### 6.2 `\\?\` and quoting

1. **`wcore-sandbox/.../windows_impl/command.rs:211,302`** — see §2.1. HIGH.
2. **`crates/wcore-config/src/shell.rs:176-178`** — HIGH. `cmd /V:ON` is applied to
   **every** hook, not only those needing the `${VAR}` → `!VAR!` rewrite in
   `hooks.rs:526-535`. Under delayed expansion `cmd` eats `!` across the whole line, so
   `echo Build complete!` or `git commit -m "fix!"` silently loses text on Windows only.
3. **`crates/wcore-memory/src/paths.rs:128,252`** — HIGH mechanism / low reach.
   `normalize_path` returns a **verbatim** path when the target exists and a **plain
   lexical** one when it does not; `Path` treats `Prefix::VerbatimDisk` and
   `Prefix::Disk` as unequal, so a mixed pair makes `starts_with` false. The doc claims
   a `dunce::canonicalize` fallback — **`wcore-memory` has no `dunce` dependency.**
   Only unit tests call it today, and they pre-create both paths, so it is a latent
   public-API trap rather than a live break.
4. Four `tempfile::persist` sites bypass `wcore-config`'s own `long_path_safe_dest`
   (`atomic_io.rs:55-68`), which exists because `MoveFileExW` returns
   `ERROR_PATH_NOT_FOUND` past 260 chars: `wcore-config/src/env_file.rs:118`,
   `wcore-agent/src/tool_backends/tts.rs:289`, `voice_mode.rs:516`,
   `wcore-agent/src/spawner.rs:406`. The `env_file.rs` one is in the same crate as the
   helper.
5. **Structural:** there are **three** private strip-verbatim helpers
   (`wcore-agent/src/session_journal/lease.rs:154`,
   `wcore-swarm/src/worktree_paths.rs:44`,
   `wcore-sandbox/.../command.rs:253`) and `dunce` is a dependency of 2 of 57 crates.
   Promoting one helper to a shared crate is the highest-leverage structural fix in
   this family.

### 6.3 Families that came back clean

- **CRLF (F3): closed.** `.gitattributes` forces `* text=auto eol=lf` plus explicit
  rules for `*.rs`, `*.toml`, `*.md`, `*.yml`, `*.snap` and the `wayland-ijfw`
  snapshots. A scan of every tracked blob under `crates/` for CR bytes returned zero.
  The `LF will be replaced by CRLF` warnings visible in the Windows test output come
  from git repos the **swarm tests create at runtime**, which have no `.gitattributes`
  — noise, not the checkout.
- **File locking (F4): no confirmed defects.** Every publish path either closes the
  handle before renaming or documents why it doesn't. (W6 is a *directory* removal
  problem, not one of these.)
- **Separator/casing (F2): nothing critical.** `wcore-repomap/src/scope.rs:167` is an
  explicit, documented normalisation boundary.

---

## 7. Process findings

1. **`skipped` still reads as `passed`, and the guard that was added did not hold.**
   `ci.yml:254` records this exact failure being diagnosed on 2026-07-27 and fixed by
   moving the contract check *after* the tests. It recurred through an earlier step.
   Step ordering is now a load-bearing property of that workflow. The durable fix is
   not another reordering — it is a job-level assertion that the test step actually
   ran. The `report` job already tries this
   (`::error title=NO TEST SIGNAL::The ci matrix produced zero nextest JUnit reports`)
   but the Windows leg's `Upload nextest JUnit report` step is `if: always()` and
   reports `success` while uploading nothing, so the assertion never fires for it.

2. **A "pass" that takes 0.016s is not a pass.** In my run,
   `release_binary_smoke::release_binary_ready_event_advertises_plugin_capabilities`
   PASSED in 0.016s — because `release_binary_or_skip()`
   (`crates/wcore-cli/tests/release_binary_smoke.rs:112-145`) returns `None` and the
   test `return`s early when the release artifact is missing, and I had not set
   `WCORE_SMOKE_REQUIRE_PREBUILT=1`. **Those two tests are NOT measured by run B.** They
   are real failures in run A (`"release child closed stdout before emitting Ready"`,
   W9) and I am not counting them as fixed.

3. **The nightly Windows soak is still reporting SUCCESS on `main`** while its value as
   a gate is unestablished. I did not re-derive this (it is already recorded in the
   handoff and is owned elsewhere), but note that `scripts/wayland-e2e-windows-soak.ps1`
   Phase G (`:287-300`) runs `cargo nextest run` on six crates with **no
   `--no-tests=fail`**, while Phase L (`:176-186`) explicitly captures `$suiteExit`
   with a comment about the array-truthiness trap. Phase G did not get the same
   treatment.

---

## 8. Reproduce any of this

```bash
gh auth switch --user FerroxLabs
# the only CI run that ever enumerated Windows:
gh api repos/FerroxLabs/wayland-core/actions/jobs/90424728470/logs > win81.log
grep 'Summary \[' win81.log
# the tip, still skipping:
gh api repos/FerroxLabs/wayland-core/actions/jobs/91165565837 \
  -q '.steps[] | "\(.conclusion) \(.name)"'
```

On the box (`ssh SeanD@seandesktop`), work under `D:\wintriage\`, never
`C:\actions-runner-*`. sshd kills the process tree on disconnect, so long runs must be
launched detached and polled from a log:

```powershell
Invoke-CimMethod -ClassName Win32_Process -MethodName Create `
  -Arguments @{CommandLine='cmd /c D:\wintriage\runtests.cmd'; CurrentDirectory='D:\wintriage\repo'}
```

Full-workspace build there is ~5m20s warm; the whole suite ~4m after that.
