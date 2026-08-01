# WINDOWS TRIAGE — 2026-07-31

Lane: `win-triage` (`lane/win-triage`). Nothing here is pushed, merged, PR'd or closed.

> **UPDATE 2026-08-01 (`lane/win-fix`) — read §9 at the bottom before using §2 or §5.**
> Windows CI still ran ZERO tests at the tip; the clippy blocker was replaced by a
> different one (§9.0, now fixed). W14's ~20 were already closed before this lane
> started and need nothing from the runner (§9.1). **W9 is still open** — its
> filed mechanism is wrong and the replacement is only partly identified
> (§9.1a). Measured on the box: **36 -> 33** failures, not 40.

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

---

# 9. UPDATE — 2026-08-01, lane `win-fix`

Everything above stands except where this section says otherwise. Three things
in it were wrong, and the two that mattered most were wrong in the same way:
a Windows-only *test hermeticity* hole was read as a product defect.

## 9.0 Windows CI still runs ZERO tests

The brief handed to this lane said run `30652437749` "reached its test step for
the FIRST TIME" and "reports real failures now". It reached the step. It ran no
tests.

```
gh api repos/FerroxLabs/wayland-core/actions/jobs/91228704700 -q '.steps[]|"\(.conclusion) \(.name)"'
  success  Clippy (warnings = errors)        <- the §3.1 fix held
  failure  Run tests (nextest CI profile)    <- died in 0.4s, zero tests
  skipped  everything after
```

```
2026-07-31T18:03:13.44Z scripts/fd-budget.sh vx cargo nextest run --workspace --profile ci --no-fail-fast
2026-07-31T18:03:13.82Z ResourceUnavailable: Program 'fd-budget.sh' failed to run: ...
                        The operation attempted is not supported.
                        error: Recipe `test-ci` failed on line 47 with exit code 1
```

`justfile` sets `windows-shell := pwsh`, and pwsh cannot execute a `.sh` file.
`fd-budget.sh` has an `is_windows()` pass-through, but that arm can only run if
the script *starts*, and it never does. So the run that was read as "Windows
finally has real numbers" produced none at all — **eight consecutive Windows
runs with zero test signal, not seven.** Call it **W17**.

That this was misread is the finding, not a footnote. §7.1 predicted it: a leg
that dies before producing JUnit is indistinguishable in the checks list from a
leg whose tests ran and failed. It has now happened again, to a reader who had
this document in hand.

**Fixed** — `justfile` gets `[unix]` / `[windows]` recipe pairs for `test` and
`test-ci` (just 1.48.1, pinned in `vx.toml`, supports the attributes).

**Ablation, measured on the box at `D:\wintriage\repo`:**

```
ABLATED (single pre-fix recipe restored):
  scripts/fd-budget.sh vx cargo nextest run --workspace --profile ci --no-fail-fast
  ResourceUnavailable: Program 'fd-budget.sh' failed to run: ... not supported
  error: Recipe `test-ci` failed on line 65 with exit code 1
  ABLATED_EXIT=1  ELAPSED_MS=1492        <- 1.5s, zero tests, byte-identical to CI
RESTORED:
  vx just --dry-run test-ci
  vx cargo nextest run --workspace --profile ci --no-fail-fast
  RESTORED_DRYRUN_EXIT=0
```

The Unix leg is deliberately unchanged — verified on hetzner-dsm, same commit:

```
/root/.local/bin/vx just --dry-run test-ci
scripts/fd-budget.sh vx cargo nextest run --workspace --profile ci --no-fail-fast
```

so the fd-budget guard still runs everywhere it can actually do something, and
`just` accepts the attribute pair on both platforms.

**Also fixed: the gap that let it read as a test failure.** `ci.yml` gains a
per-leg step after the test step (`if: !cancelled()`, `shell: bash`) asserting
(1) `target/nextest/ci/junit.xml` exists and (2) it declares `tests > 0`, with
`::error title=NO TEST SIGNAL (<leg>)` / `ZERO TESTS (<leg>)`. The existing
`report`-job assertion cannot do this: it fires only when *no leg anywhere*
produced JUnit, so macOS uploading a report hides a dark Windows leg completely.

This gate is UNVERIFIED IN CI — this lane cannot push, so it has never run in a
real job. It is checked only by `yaml.safe_load` locally (parses; the step lands
immediately after `Run tests`, `if: ${{ !cancelled() }}`, `shell: bash`,
`LEG: ${{ runner.os }}`). `runner.os` and not `matrix.os` on purpose: the Windows
matrix entry is a label LIST, which interpolates to the literal string `Array`.

## 9.1 W9 and W14 are ONE root cause, and it is not the one filed

`HOME` is not an isolation mechanism on Windows. Only `WAYLAND_HOME` is.

`wayland_config_dir()` (`wcore-config/src/config.rs:3367`) resolves
`WAYLAND_HOME` → `XDG_DATA_HOME` → `dirs::config_dir()`. On Windows the last one
is the `FOLDERID_RoamingAppData` known folder, read from the OS; `HOME` is never
consulted. So a test that spawns `wayland-core` with `.env("HOME", tmp)` and
`.env_remove("WAYLAND_HOME")` — believing it has an empty profile — actually
runs against **the invoking account's real `%APPDATA%\wayland-core`**.

Both accounts on the box carry a profile there, and both set the same thing:

| account | config | `[storage.credentials]` | `[session]` |
|---|---|---|---|
| `seand` (interactive) | `C:\Users\seand\AppData\Roaming\wayland-core\config.toml:104` | `backend = "plaintext"` | `enabled = true` |
| `NT AUTHORITY\NETWORK SERVICE` (the CI runner) | `C:\Windows\ServiceProfiles\NetworkService\AppData\Roaming\wayland-core\config.toml:109` | `backend = "plaintext"` | `enabled = true` |

`reject_backend_without_confidential_storage` refuses that combination **by
design** — plaintext cannot hold the confidential key durable recovery needs,
and `durable_sessions_must_be_disabled` deliberately does NOT degrade it
(`config.rs:2594-2597`: an operator who chose plaintext must keep hearing about
it). So the engine emits `init_failed` instead of `ready`, and every test that
waits for `ready` fails.

Nothing is broken in the engine. Nothing is unavailable on the runner.

**Measured on the box 2026-08-01, same binary, minutes apart:**

```
# ambient profile (what the tests actually got)
{"type":"error","error":{"code":"init_failed","message":"Engine failed to start:
 storage.credentials.backend is set to \"plaintext\", ..."}}   EXITCODE=1

# WAYLAND_HOME pinned to an empty directory
{"type":"ready","version":"0.12.25","capabilities":{...,"user_model_backend":"local",...}}
```

**And the same pair as the CI service account itself**, via a `schtasks
/RU "NT AUTHORITY\NETWORK SERVICE"` probe (nothing under `C:\actions-runner-*`
was touched):

```
whoami -> nt authority\network service
  ambient profile        -> init_failed, EXITCODE=1
  WAYLAND_HOME pinned    -> {"type":"ready",...,"session_id":"b6be1c690c4f",...}, EXITCODE=0
```

So:

* **W9 is not "`--json-stream` never emits `ready` on Windows".** The engine
  emits it in about three seconds, measured repeatedly. But the config read is
  not the whole of W9 and **W9 is NOT closed** — see §9.1a before doing anything
  with this row.
* **W14 is not "credential vault unavailable to the CI service account", and it
  is already closed — by work that predates this lane.** The vault is fine: the
  service account opens a confidential store and gets a `session_id`. The ~20
  CI failures came from `deterministic_openai_loop` / `smoke_p0` /
  `acp_gate_d012`, and those families **already pin `WAYLAND_HOME`** (via
  `wcore-eval-scenarios::runner::run_with_binary` passing `Some(env.home())`,
  and `smoke_p0.rs:164`). What refused in run A was the ISOLATED-profile
  confidential path on a keyring-less account, and that was fixed on 2026-07-30
  by `c73ac417` ("a host with no keyring now runs, degraded and announced,
  instead of dying"), two days AFTER run A was measured.
  **This lane's contribution to W14 is the measurement, not the fix.** The
  brief's framing ("runner *configuration*... may genuinely need an interactive
  login") is refuted: no login, no runner change, no `icacls` on the runner, and
  nothing to ask Sean for.
* The runner's ambient profile at
  `C:\Windows\ServiceProfiles\NetworkService\AppData\Roaming\wayland-core\`
  contains `sessions\`, `memory\memory.db`, `channels\channel-poll.lock`,
  `projects\...` — **written by previous CI runs that believed they were
  isolated.** Windows CI has been accumulating shared cross-run state. That is
  also the most likely explanation for the run-A/run-B differential in §1.5 that
  this document used as its primary classifier: 55-only-in-A and 14-only-in-B is
  what two different accumulated profiles look like.

### 9.1a W9 IS NOT CLOSED — the fix does not hold, and I could not isolate why

This is the part of this update that a reader must not skim.

The mechanism in §9.1 is real and is proven at the binary level. Pinning
`WAYLAND_HOME` **does not reliably turn r012 green**, and I ran out of budget
before finding out what else is in the way. Sequence, in order, all on the box:

| # | what | result |
|---|---|---|
| 1 | r012 alone, fix applied | **PASS [3.634s]** |
| 2 | r012 alone, fix ablated | FAIL [2.370s] — no `user_model_backend` (§9.1) |
| 3 | r012 alone, fix restored | (not re-run at this point — my mistake) |
| 4 | full suite, fix applied | **FAIL [33.567s]** — `no ready event within 10s` |
| 5 | r012 alone, fix applied, after the full suite | **FAIL [2.401s]** — plaintext `init_failed` again |

Step 5 is the one that matters: the same test, same command, same tree that
passed at step 1, now fails the way it failed BEFORE the fix. So step 1 is not
reproducible and cannot be reported as a pass.

What I did establish, and re-established four times after step 5, is that the
ENGINE is fine. Driving `target\debug\wayland-core.exe` directly with the exact
r012 environment — `WAYLAND_HOME` pinned inside a `%TEMP%` tempdir, `HOME` set,
cwd = tempdir, `--yolo --json-stream` — emits `ready` every time:

```
with the seeded .wayland-core/config.toml     FIRST_TYPE=ready
without it                                    FIRST_TYPE=ready
launched from ssh                             FIRST_TYPE=ready
launched detached via Win32_Process (the way nextest runs)  FIRST_TYPE=ready
  notice: durable session persistence is OFF for this run.   <- c73ac417's degrade, working
```

So the engine takes the degrade path and starts. Something on the **nextest test
path specifically** still reaches the ambient `%APPDATA%\wayland-core\config.toml`.
Ruled out by measurement, not by argument: the binary (`CARGO_BIN_EXE_wayland-core`
resolves to the same `target\debug\wayland-core.exe`; no release artifact exists
and `WCORE_EVAL_BIN` is unset), the seeded project config, the cwd-walk (no
`.wayland-core` on any ancestor of `%TEMP%`), and the launch context's ambient
environment.

Two candidate explanations I did NOT get to test, in the order I would test them:
1. something in r012's **Layer 0** (the eval-scenarios runner, which passes and
   runs first in the same test) leaves process or filesystem state that Layer 1
   inherits;
2. an env var nextest sets that reaches config resolution ahead of
   `WAYLAND_HOME`.

Step 4 adds a probable SECOND cause on top: in the full suite the failure is a
clean `no ready event within 10s` (~10.5s x 3 attempts, no output at all),
whereas alone the engine answers in ~3s. A wall-clock deadline inside a
`test-threads = num-cpus` suite is flaky by construction — but bumping the
number is exactly the change that reads as a fix while hiding a real startup
regression, so it is not done here.

**W9 STAYS OPEN.** The `WAYLAND_HOME` pin is kept because it is strictly correct
(§9.1 proves `env_remove` was wrong) and because r012 was already red before it,
so it costs nothing — but it is **not** a fix and the failure count in §9.3a
does not credit it. ~1 day: reproduce step 5, bisect Layer 0, then deal with the
deadline.

**Applied, but NOT claimed as a fix (the config half).** `harness_regression.rs` r012 pins `WAYLAND_HOME`
instead of removing it. A new contract test,
`wcore-config config::tests::home_alone_isolates_on_unix_and_does_not_isolate_on_windows`,
pins the platform fact on BOTH arms so the trap cannot go quiet again.

### 9.1b W9's root cause, measured — a 15s sandbox probe on the startup path

Superseding the two "candidate explanations I did not get to test" in §9.1a.
Both were wrong, and one of the facts §9.1a rests on does not reproduce.

**Step 5 does not reproduce.** Same box, tree at `1d032188` + this lane's
instrumentation, `D:\w9`, `cargo nextest run -p wcore-cli --no-fail-fast`
(2400 tests, 151s), then r012 alone immediately after:

| # | what | result |
|---|---|---|
| 1 | r012 alone, cold | **PASS [4.731s]** |
| 2 | full `wcore-cli` suite | r012 **FAIL [31.466s]** |
| 3 | r012 alone, straight after the full suite | **PASS [2.285s]** |

So the `WAYLAND_HOME` pin DOES hold. §9.1a's step 5 ("plaintext `init_failed`
again") is not a property of this tree. A direct measurement of what an isolated
engine launch touches confirms it: snapshotting every file under
`%APPDATA%\wayland-core`, `%APPDATA%\wayland-core-profiles`,
`%LOCALAPPDATA%\wayland-core`, `%USERPROFILE%\.wayland` and
`%USERPROFILE%\.wayland-core` around one `--json-stream` launch with
`WAYLAND_HOME` pinned reports **`(none)` created, modified or deleted**. Nothing
on the nextest path reaches ambient config. That hypothesis is closed.

**The real cause was invisible because r012 discarded it.** The ready-event
layer spawned the engine with `.stderr(Stdio::null())`, so a timeout produced
the words "no ready event" and nothing else — which is precisely why two lanes
could not name the mechanism. r012 now pipes and drains the engine's stderr and
prints it, with the measured elapsed time, in the failure. The first run with
that change says it outright:

```
R-012 FAIL: no ready event on stdout within 10s (waited 10.014159s)
engine stderr:
  ...
  2026-08-01T03:25:55.938146Z  WARN not advertising browser_suite: ...
  2026-08-01T03:26:10.942634Z ERROR AppContainer probe exceeded its hard
    wall-clock guard — a Win32 setup call (CreateAppContainerProfile /
    CreateProcessAsUserW) stalled, most likely an AV image scan or
    profile-service RPC. ... guard_secs=15
```

`03:25:55.938` → `03:26:10.942` is 15.004s: the probe's own guard, burned in
full, inside engine startup, before `ready`.

**The chain, in source:**

1. `crates/wcore-agent/src/bootstrap.rs:1079` — every session resolves its
   sandbox runtime via `SandboxRegistry::required_for_session(...)` during
   bootstrap, before `ready` is emitted.
2. `crates/wcore-sandbox/src/lib.rs:407` → `real_platform_backend()`
   (`lib.rs:651`) → `lib.rs:672` `AppContainerBackend::is_available()`.
3. `crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs:288`
   `probe_appcontainer_available()` — a REAL `cmd.exe /c exit 0` through the
   whole AppContainer pipeline, bounded by `PROBE_WALL_CLOCK` = **15s**
   (`process.rs:314`).

**Why 15s is the whole bug, not bad luck.** The guard is longer than r012's 10s
ready deadline, so any run that reaches the guard misses the deadline
deterministically — there is no threshold at which the test could have been
lucky. Idle, the same launch reaches `ready` in **750 ms** and r012 finishes in
2.2s, so this is not gradual degradation under load: it is a hard stall in
`CreateAppContainerProfile`, which is an RPC into a Windows global service that
dozens of concurrent engine spawns serialize on. `probe_gate()` single-flights
the probe WITHIN a process; nothing coordinates ACROSS processes, so a 2400-test
suite pays it once per engine child.

**It is not only r012.** The same 15s signature accounts for most of the
Windows suite's remaining reds, all of which spawn an engine and all of which
die at the guard, not at their own logic:

```
TRY 2 FAIL [15.044s] wcore-cli::tool_formatter_real_payloads bash_stderr_is_surfaced
TRY 2 FAIL [15.036s] wcore-cli::tool_formatter_real_payloads bash_success_renders_the_real_exit_code_and_byte_count
TRY 2 FAIL [15.035s] wcore-cli::tool_formatter_real_payloads bash_failure_never_reports_exit_zero
TRY 2 FAIL [15.083s] wcore-cli::sandbox_activeness sandbox_exec_confines_a_write_that_escapes_the_workspace
```

**And it is a product defect, not a test defect.** A Windows user whose AV is
mid-scan waits 15 seconds for `ready` on every launch, and Wayland Desktop
consumes `ready` before it can show anything.

**NOT FIXED HERE, and why.** The fix is to take the probe off the readiness
path: resolve the platform backend lazily behind a once-only cell so the
verdict is computed at first sandboxed execution rather than at bootstrap. That
is safe on the fail-closed axis — `required_for_session` already substitutes
`FailClosedBackend` rather than refusing to start (`lib.rs:407`), so deferral
does not turn a refusal into a silent bypass. It is NOT safe to land blind,
because `bootstrap.rs:2909` puts `sandbox_runtime().backend_name()` into the
`WorkspacePolicyReceipt` a host reads, and `bootstrap.rs:2686`/`:2805` read
`enforces_read_deny()` for channel tool posture. Deferring the probe therefore
requires deciding what a host is told about the backend before the verdict
exists — a protocol-visible decision on a security boundary, on the one platform
that cannot be exercised locally. Raising r012's deadline past 15s would hide
exactly this, so it is not done: `READY_DEADLINE` stays at 10s and now carries
the measurement that justifies it.

Only two `env_remove("WAYLAND_HOME")` sites exist in the workspace; the other
(`json_stream_startup_refusal.rs:242`) has absent-`WAYLAND_HOME` as its
condition under test and is correct. The packaged-CLI families
(`smoke_p0`, `acp_gate_d012`, `wcore-eval-scenarios::runner`) already pin it.

**Ablation, both directions, on the box:**

```
FIXED
  wcore-cli::harness_regression r012_honcho_fallback_on_no_key   PASS [3.634s]
  wcore-config ...home_alone_isolates_on_unix_and_does_not_isolate_on_windows
                                                                  PASS [0.014s]
ABLATED  (.env("WAYLAND_HOME", &isolated_home) -> .env_remove("WAYLAND_HOME");
          the unit test's Windows arm inverted)
  r012   TRY 3 FAIL [2.370s]
         "R-012 FAIL: ready event carries no capabilities.user_model_backend"
  unit   TRY 3 FAIL [0.011s]
         "HOME appears to relocate the config dir on Windows
          (got C:\Users\seand\AppData\Roaming\wayland-core)"
```

That second line is the mechanism printed by the instrument itself: with
`WAYLAND_HOME` and `XDG_DATA_HOME` removed and `HOME` pointed at a tempdir,
`wayland_config_dir()` still answers with the real roaming profile.

One honest discrepancy: in the ablated arm r012 fails on the *field* assertion
in 2.4s, whereas in the §9.3 full-suite baseline it failed on the *10s ready
timeout* in 31.5s. Same cause — the first stdout line is an `init_failed` frame
rather than `ready` — but under full-suite load the engine did not get that
frame out inside the window, and alone it does. The assertion that fires differs;
the root cause does not.

The Linux leg runs the SAME unit test and passes it through the opposite arm
(`cargo nextest run -p wcore-config`, hetzner-dsm: `1 passed`), which is what
makes it a cross-platform contract rather than a Windows quirk.

## 9.2 W3 is a test-fixture defect, not a security decision

§5 rank 5 said W3 needed "someone who knows whether the intended ACE should
include the writer's own SID — a correctness question about the security
property". It does not. The production path is untouched by this.

`set_identity_bound_file_dacl` (`snapshot.rs:755`) closes a TOCTOU window by
re-opening the path after the write via `ensure_path_identity` →
`open_identity_probe`, which opens with `read(true)`. Windows grants a file's
OWNER `READ_CONTROL` and `WRITE_DAC` implicitly whatever the DACL says — which
is why the `open_security_handle` *before* the write succeeds — but grants no
implicit `GENERIC_READ`. The four `#[cfg(test)]` hostile-DACL installers reuse
that production helper, so the instant they install `Deny Everyone` or an empty
DACL, their own post-write read re-probe returns `ERROR_ACCESS_DENIED` and the
INSTALLER panics. Both tests therefore died before reaching the
`validate_private_file` assertion they exist to make — they have never asserted
anything on Windows.

Production only ever installs `Allow <this user>`, which the writer can always
reopen. `secure_private_file` is unchanged.

**Fixed** — a `#[cfg(test)]` `set_hostile_file_dacl` that omits the re-probe,
plus `release_hostile_dacl` which hands the file back before `NamedTempFile::drop`.

The second half is belt-and-braces, and I checked rather than assumed: after the
ablated run left two files carrying a live `Deny Everyone` DACL,
`%TEMP%` held **0** leftovers. Deleting a file needs `DELETE` on the file **or**
`FILE_DELETE_CHILD` on its parent, and a user has the latter on their own
`%TEMP%`. So the accumulation I wrote `release_hostile_dacl` to prevent does not
occur here. It still guards a fixture pointed at a directory where that is not
true, which is why it stays — but the claim is "defensive", not "load-bearing".

**Ablation, both directions, on the box:**

```
FIXED     3 tests run: 3 passed
          windows_private_dacl_rejects_unprotected_inheritance      PASS
          windows_private_dacl_accepts_restrictive_deny_ace         PASS
          windows_private_dacl_rejects_null_empty_and_broad_allow   PASS
ABLATED   (post-write `ensure_path_identity` put back in the hostile installer)
          3 tests run: 1 passed, 2 failed
          accepts_restrictive_deny_ace / rejects_null_empty_and_broad_allow:
            called `Result::unwrap()` on an `Err` value: Custom { kind: Other,
            error: Io { path: "C:\\Users\\seand\\AppData\\Local\\Temp\\.tmp4VDmrP",
            source: Os { code: 5, kind: PermissionDenied } } }
```

The ablated error is byte-identical to the §9.3 baseline failure, which is what
says the fix addresses the measured defect and not a lookalike.

The other two W3 failures (`wcore-cli log_rotate::tests`) are NOT this and are
still open.

## 9.3 The new measured baseline

Full workspace, real box, interactive user, branch tip `5d1eda16`:

```
vx cargo nextest run --workspace --profile ci --no-fail-fast
Summary [222.350s] 13350 tests run: 13314 passed (2 slow, 1 flaky, 1 leaky), 36 failed, 130 skipped
```

**36, not 40** — §4's W1/W13 fixes hold. Composition of the 36:

| root cause | count | note |
|---|---|---|
| **W6** swarm worktrees never reclaimed | 9 | `dispatch_smoke` ×4, `worker_runtime_limits` ×2, `worktree::tests` ×2, `swarm_worker_failure_reporting_e2e` ×1 — now the largest single block |
| **W5** descendants not reaped | 4 | `runner_contracts::*reaps_owned_descendant_listener` |
| *(new)* `tool_formatter_real_payloads` | 4 | not in §2 at all; 3 of the 4 are `bash_*` |
| **W3** DACL | 4 | 2 fixed here; `log_rotate` ×2 still open |
| **W1** `/`-rooted literal | 3 | `gate_executor:652`, `journal_effects:1565`, `transcription_tools:833` — the §5 rank 6 remainder |
| **W4** AppContainer child exec | 2 | `sandbox_activeness`, `typed_execution_policy_e2e` |
| *(unfiled)* `audit_2026_05_22_tests` | 2 | filesystem-checkpoint restart, also red in run A |
| **W12** read-timeout attempt count | 2 | + `packaged_f04_run_is_repeatable` |
| **W9** | 1 | still open — §9.1a |
| W7 / W8 / W10 / d1_refusal / f04 | 5 | one each |

W2 (`session_journal_compaction_test restart_rejects_*`) and the W1
`receipt_contract` block are gone from the failure set.

### 9.3a After the fixes, measured the same way

```
vx just test-ci                     # the real recipe, exercising the W17 fix end to end
Summary [283.277s] 13351 tests run: 13318 passed (2 slow, 1 flaky, 2 leaky), 33 failed, 130 skipped
```

**36 -> 33.** Attribution, honestly:

| | |
|---|---|
| `windows_private_dacl_accepts_restrictive_deny_ace` | fixed here (§9.2) |
| `windows_private_dacl_rejects_null_empty_and_broad_allow` | fixed here (§9.2) |
| `packaged_core_exhausts_a_real_read_timeout` | **NOT mine — run-to-run variance.** Its sibling `packaged_core_recovers_after_a_real_read_timeout` still fails; these two flip. Treat the suite as 33 ± 1. |
| `r012_honcho_fallback_on_no_key` | **still red, still open** — §9.1a |

So the delta this lane can actually claim on the failure count is **2**, and the
run also confirms the W17 fix end to end: `just test-ci` executed 13351 tests on
Windows instead of dying in the recipe.

The count is the least interesting result here. The two that matter are that
W14's ~20 are already closed (measured, §9.1) and that ~13350 tests now run on
Windows CI at all (§9.0).

## 9.4 What §5 should say now

| rank | root cause | count | revised cost |
|---|---|---|---|
| 1 | **W6** swarm worktrees | 9 | 1 day — unchanged, and now the biggest block |
| 2 | **W5** descendant reaping | 4 | 1-2 days, still must be done with the macOS EPERM sibling |
| 3 | `tool_formatter_real_payloads` | 4 | unmeasured — needs its own triage pass, ~2h to classify |
| 4 | **W1** remainder | 3 | 1-2 hours, mechanical |
| 5 | **W4** AppContainer | 2 | 1-2 days + security review — **still a Sean decision**, §2.1 unchanged |
| 6 | `log_rotate` W3 remainder | 2 | 2-3 hours |
| 7 | W7 / W8 / W10 / W12 | 6 | ~1 hour each |
| 8 | **W9** r012 — see §9.1a | 1 | ~1 day. NOT closed. Reproduce the step-5 regression, bisect Layer 0, then the 10s deadline |

**Deleted from §5:** rank 10 (W14) — closed, and closed before this lane
started (§9.1). **Rank 1 (W9) is NOT deleted and NOT closed** — its stated
mechanism is wrong (§9.1) and its real one is only partly identified (§9.1a).
It moves to rank 8 above with a corrected description.
**Deleted from §5:** rank 11 (W11 box contamination) — `D:\nonexistent` removed
from the box 2026-08-01; `Test-Path D:\nonexistent` now `False`.

## 9.5 What is owed that this lane could not do

* **The runner's ambient profile is still there.** Pinning `WAYLAND_HOME` in the
  tests stops new pollution and stops tests reading it, but
  `C:\Windows\ServiceProfiles\NetworkService\AppData\Roaming\wayland-core\`
  still holds the accumulated state (and the `backend = "plaintext"` config that
  caused ~20 CI failures). Deleting it is a one-line operator action on the
  runner host and is Sean's to take, not this lane's — it is outside `D:` and
  adjacent to live runner services. Nothing depends on it once the pins are in,
  but leaving it means any FUTURE unpinned test resurrects the whole class.
* **No CI verification.** This lane cannot push, so none of this has been
  observed in a real Windows CI job. Every claim above is from the box.
* **No macOS clippy.** `cargo` is forbidden on the development Macs and this
  lane had no macOS runner, so the two platforms that were actually run are
  Windows (`vx cargo clippy -p wcore-config -p wcore-agent -p wcore-cli
  --all-targets -- -D warnings` -> exit 0) and Linux (same command on
  hetzner-dsm -> exit 0). The residual risk is small and stated rather than
  hidden: the `snapshot.rs` change lives entirely inside
  `#[cfg(windows)] mod windows_snapshot_security`, and the `config.rs` test
  branches on `cfg!(windows)` at RUNTIME (both arms compile on every target),
  so there is no macOS-only arm in this diff for a macOS clippy to reach that
  the Linux run did not already compile.

## 9.6 Bonus finding — the anti-vacuity gate never runs

Not Windows, found while checking that the `justfile` change did not trip it.

`ci.yml:243` — *"No vacuous `cargo test` invocations"* — is guarded by
`if: runner.os == 'Linux'`, and it lives in the `ci` matrix, whose `os` vector is
`["macos-latest", ["self-hosted","Windows","X64","msvc"]]`. **There is no Linux
entry in that matrix** (Linux moved to the separate containerized `ci-linux`
job, which does not run this step). So the guard is never true and the step is
recorded `skipped` on every leg of every run — visible in the run-30652437749
step list quoted in §9.0.

It is not an empty gate. Run manually on hetzner-dsm at `c5ce3857`:

```
python3 scripts/check-no-vacuous-cargo-test.py --self-test   -> SELF-TEST: PASSED (6 assertions)
python3 scripts/check-no-vacuous-cargo-test.py               -> exit 1
  GATE: FAILED - 4 unguarded `cargo test` invocation(s).
  .github/workflows/macos-docker-gate.yml:119,130,149,160
```

Four real violations, in a workflow whose entire purpose is a live gate, sitting
undetected because the check that would name them is wired to a runner class the
matrix does not contain. Pre-existing — `macos-docker-gate.yml` was last touched
by `48d1b1a4`, and this lane's diff does not go near it.

Deliberately NOT fixed here: moving the step (to `ci-linux`, or dropping the
`if:` so it runs on the Windows leg — it is a ~50ms text scan and needs only
`python3`) turns the Windows job red on someone else's four lines, on a branch
whose whole point is to get Windows to a readable number. It should be one
commit, with those four fixed in it. ~1 hour.
