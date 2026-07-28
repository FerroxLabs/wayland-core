# 28-CLEANUP — the five reds disposed, macOS reached through CI, 13 guards falsified

**Branch:** `lane/28-cleanup`, off `plan/f20-unified-audit-repair` at merge-base
`926216f3c0656e38bc5bcfd4b8fb1ad84a08297f` (captured once, quoted, diffed against the SHA).
**Hosts:** `hetzner-dsm` (Linux, `/root/wayland-28cleanup`), GitHub-hosted `macos-latest`.
**Never merged into the integration branch. No PR, no tag, no issue closed.
`wcore-contract generate` NOT run. Neither shared-fence file touched.**

Predecessor spec: `F-KR-07-SUITES-SUMMARY.md` (`lane/kr07-suites`, merged at `926216f3`).

---

## Target 1 — the five reds, each with a terminal disposition

### How severity was scored

Phase 28 Success Criterion 1, verbatim:

> 1. Native macOS, Linux, and Windows pass the required hostile platform matrix with no
>    skipped critical case.

Under **Amendment A2** a finding that contradicts a Success Criterion is CRITICAL/HIGH *by
construction* and may only be FIXED or DISPROVED. The question each ladder had to answer first
is therefore **"is this suite in that matrix?"** — because that decides which dispositions
even exist.

The hostile platform matrix is the native `wcore-sandbox` live acceptance surface
(`live_fs_acl`, `hard_process_containment_{windows,macos}`, `live_cwd_verbatim`,
`live_integrity_macos`). **None of the three red suites is in it.** `actor_acl_test` is
platform-agnostic agent orchestration; `tool_token_bench_smoke` is bench-class; `acp_engine_turn`
is protocol-bridge. So A2 does not fire by construction for any of them, and accept/defer stay
open — but all three were still driven to FIXED or DISPROVED rather than deferred, because each
turned out to be attributable.

---

### RED 1 — `wcore-agent/actor_acl_test` (4 passed / 1 FAILED) → **DISPROVED**

Reproduced verbatim on hetzner (run A, serial, `-- --ignored`):

```
running 5 tests
test sub_agent_with_deny_policy_short_circuits ... FAILED
panicked at crates/wcore-agent/tests/actor_acl_test.rs:153:13:
expected deny error result; got success: tool-executed
test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

Denial ladder, one property per rung, each a separate observation:

| Rung | Property isolated | Result |
|---|---|---|
| 1 | Does the asserted enforcement string exist in the product? | `"Denied by sub-agent learned policy"` occurs in **exactly one file in the workspace — the test itself**. Zero product sources. |
| 2 | Is `CallActor::SubAgent` ever constructed in production? | Only in `wcore-permissions/src/actor.rs` unit tests (lines 78, 92) and in two comments. **No production construction site.** |
| 3 | Is `learned_policy` ever `Some` in production? | Every production construction site sets `learned_policy: None` — `node_executor.rs:420`, `engine.rs:11421`. |
| 4 | Are the 4 greens meaningful? | **No.** All four assert *"the tool runs"*, which is trivially true when no pre-filter exists. The suite is 4 vacuous greens plus 1 honest red. |

The pre-filter was deliberately removed in v0.8.1 U11 and the file's own header says so, retaining
the tests as a spec for a future wave. Rungs 1–3 confirm that claim independently rather than
trusting the prose. The unenforced deny path is **unreachable in production**, so it is not a live
security gap, and the red is an artifact of forcing a documented forward spec to execute under
`--ignored`.

**Left byte-identical.** No `#[ignore]` removed, no assertion relaxed, no test deleted.
`28-04` should treat this suite as a forward spec, not a certification input.

---

### RED 2 — `wcore-agent/tool_token_bench_smoke` (0 passed / 1 FAILED) → **DISPROVED** as a product defect; harness gap filed MEDIUM

Reproduced: `0 passed; 1 failed`, 63.14 s.

The gate printed `- Bash / echo hello: error` and nothing else, telling the operator to *"see
scratch workdir for details"* while `cleanup_workdir` deleted that directory **on the same code
path**, and `Row` never retained the content in the first place. The failure was undiagnosable by
construction. Fixed that first (commit `08f7b5bd`) — additive, no assertion touched — which made
the rest of the ladder possible.

| Rung | Property isolated | Result |
|---|---|---|
| 1 | What is the actual error? | `sandbox child execution failed: sandbox UNAVAILABLE and unsandboxed execution is not permitted — refusing to run with host permissions.` |
| 2 | Can this host sandbox at all? | **Yes.** `/usr/bin/bwrap` present; `bwrap --ro-bind / / --dev /dev /bin/echo` → rc 0; `unshare --user --map-root-user` → rc 0; `max_user_namespaces=1029399`. |
| 3 | Is bwrap visible to the test's own environment? | **Yes** — `which_bwrap=/usr/bin/bwrap` captured inside the identical shell that ran the test. So `real_platform_backend()` would return bwrap. |
| 4 | Did a *selection* happen at all? | **No.** `WAYLAND_ALLOW_NO_SANDBOX=1` produced a byte-identical failure. That env var is read only by `unsandboxed_fallback()`, so the code never reached the selection chokepoint — a `FailClosedBackend` was already in place. |
| 5 | Where from? | The bench source contains **zero** occurrences of `Sandbox` or `FailClosed`. It builds a bare `ToolRegistry` with no engine. `BashTool` takes its backend from `ctx.sandbox` (`bash.rs:485`, `:567`). |

The product's fail-closed refusal is correct and is exactly the audit M-2 / rel-concurrency-70
design. **The bench never wires a sandbox backend into its dispatch context**, so `BashTool` is
structurally unable to execute there. Not a platform defect, not in the matrix.

| ID | Severity | Finding |
|---|---|---|
| **F-28C-01** | **MEDIUM** → BACKLOG | `tool_token_bench` cannot measure `BashTool` on any host: it dispatches through a context with no sandbox backend, so every Bash row fails closed and the sanity gate refuses to write the markdown. The bench's Bash column has therefore never been produced. Non-blocking; bench-class. |

---

### RED 3 — `wcore-cli/acp_engine_turn` (0 passed / 2 FAILED) → **DISPROVED** as a product defect; suite still red, reported red

Reproduced: both cases fail identically at `engine init_session`.

| Rung | Property isolated | Result |
|---|---|---|
| 1 | What is the refusal? | `credentials.backend is set to "plaintext", which cannot hold the confidential key that durable session recovery requires` — `RecoveryConfidentialError::PlaintextBackendRejected`. |
| 2 | Can defaults produce `plaintext`? | **No.** `CredentialsStorageConfig::default()` is `Auto` (asserted by the `default_backend_is_auto` unit test), and `supports_confidential_material()` is `!matches!(self, Plaintext)`, so `Auto` satisfies it. Something must be *supplying* plaintext. |
| 3 | What supplies it? | `hetzner-dsm:~/.config/wayland-core/config.toml` **line 103: `backend = "plaintext"`**. The test calls `Config::resolve(&CliArgs{..})`, which reads the operator's real config — despite the file's doc-comment calling this "the hermetic test seam". |
| 4 | **Decisive** — remove the host config. | Re-run under isolated `HOME` + `XDG_CONFIG_HOME`: the plaintext error **disappears** and the panic **moves from line 87 to line 98**. Attribution complete: the original red was host-config contamination. |
| 5 | What remains once isolated? | A different error: `secure recovery storage is unavailable: no OS keyring was usable and no encrypted credentials vault is unlocked`. A headless Linux box has no keyring. |

Rung 4 is the load-bearing one — the panic *moving line* is what attributes the failure rather
than guessing at it. The product's refusal at rung 5 is deliberate, documented, fail-closed and
carries two actionable remediations; it is not a defect. **The test is non-hermetic and
environment-dependent**, which is the real fault.

**The suite is still red and I am reporting it red.** I did not make it pass: doing so would have
meant configuring credentials inside the test, which is a change to what it asserts.

| ID | Severity | Finding |
|---|---|---|
| **F-28C-02** | **MEDIUM** → BACKLOG | `acp_engine_turn` reads the host's real `config.toml` while documenting itself as hermetic, so its result depends on the developer's machine. It also cannot pass on any headless Linux host without explicit credentials configuration. |
| **F-28C-03** | **MEDIUM** → BACKLOG | An ACP/A2A session cannot be established on a headless Linux host with no OS keyring unless the operator sets `credentials.backend = "encrypted-file"` and supplies a passphrase. Fail-closed and actionable, so not a security defect — but headless Linux is the canonical deployment for an agent CLI and this is a first-run wall. |

*(Counting: the predecessor's "five reds" are the 1 + 1 + 2 failing tests above plus the two
Flavour-B suites it confirmed. The Flavour-B pair is unchanged and remains as it reported them.)*

---

## Target 2 — macOS: CI **can** reach it, and the false blocker is retracted

The predecessor's "no macOS build host in this lane's reach" is **wrong**, and I am recording that
plainly rather than re-filing it. `ci.yml:75` and `:417` both use `macos-latest`, and `release.yml`
builds both Darwin targets. A GitHub-hosted macOS runner is a macOS build host.

Added `.github/workflows/macos-native-suites.yml`, which runs both
`required_live_macos_*` cases on `macos-latest` with `WAYLAND_SANDBOX_LIVE_MACOS=1` and
`-- --ignored --test-threads=1`.

**Why its gate can fail.** Both suites open with two early returns — unset env, and unavailable
backend — each printing `skip:` and returning, so an un-opted-in host reports `1 passed` having
asserted nothing. Asserting exit status, or even `1 passed`, would certify nothing. The gate
asserts three properties separately, and the third is load-bearing:

1. `running 1 test` — the case was collected, not filtered away (closes flavours A and C);
2. `1 passed; 0 failed`;
3. **no `skip:` line in stdout** — neither early return was taken (closes flavour B).

Exit codes are taken from `PIPESTATUS[0]`, never off the tail of a `| tee`, because a pipe steals
exit status. The gate is a separate step from the run steps so a non-zero cargo exit cannot
short-circuit the count checks.

Two runs were dispatched on `lane/28-cleanup`: **30362524522** and **30363073170**.

**Honest status: at the time of writing both are still `queued` — no macOS runner had picked
them up ~50 minutes after dispatch, across 9 polls.** I have therefore **not** obtained a macOS
result, and I am not claiming one.

The cause is *not* the macOS runner class, and I checked rather than assumed. The repository's
whole Actions queue is saturated while five lanes push concurrently: `ci.yml` runs
`30363995482` and `30363993960` were `pending` at the same moment, and `30363828443` /
`30363077139` were `cancelled` outright. This workflow deliberately declares no `concurrency`
group, so it is not being cancelled — only queued behind everything else.

So the blocker moved from *"no such machine exists"* (false) to *"a queue wait on known-present
hardware"* (measured). **`28-04` should read runs 30362524522 / 30363073170 — they will complete
without further action.** If the queue is still saturated then, re-dispatch is a single
`gh workflow run` once this branch reaches the default branch.

One mechanical finding worth carrying forward: `gh workflow run` returns
`HTTP 404: workflow not found on the default branch`, because the REST API resolves a dispatchable
workflow by name off the **default branch**. A workflow that exists only on a topic branch cannot
be dispatched by name. The scoped `push` trigger is what actually lets it run pre-merge.

---

## Target 3 — guards, flavour (d), and the detector's own falsification

### The guard table — 13 added, each falsified rather than asserted

The predecessor guarded 3 of 16. The remaining **13** now carry an always-running
`zero_execution_guard` in the established idiom. It is deliberately **not** `#[ignore]`d: three
suites in this repo carried a guard that was itself ignored and so was inert against precisely its
own scenario.

Falsification measured on hetzner, not asserted — env set without `--ignored` must be red, env
unset must be green:

| Suite | `WAYLAND_REQUIRE_IGNORED=1`, no `--ignored` | env unset | Before |
|---|---|---|---|
| `wcore-exec-backend/live_equivalence` | `test zero_execution_guard ... FAILED` **rc=101** | `ok. 1 passed; 0 failed; 1 ignored` **rc=0** | `ok. 0 passed; 1 ignored` |
| `wcore-memory/hybrid_retriever_perf_test` | `FAILED` **rc=101** | `ok. 1 passed; 0 failed; 2 ignored` **rc=0** | `ok. 0 passed; 2 ignored` |
| `wcore-eval-scenarios/pty_tui_smoke` | `FAILED` **rc=101** | `ok. 1 passed; 0 failed; 1 ignored` **rc=0** | `ok. 0 passed; 1 ignored` |

Remaining 10 guarded identically by the same generator: `actor_acl_test`,
`tool_token_bench_smoke`, `acp_engine_turn`, `acceptance_gate`, `cross_session_live`,
`live_personas`, `bge_local_real`, `otlp_local_test`, `hard_process_containment_macos`,
`live_integrity_macos`. The two macOS suites additionally honour their own pre-existing
`WAYLAND_SANDBOX_LIVE_MACOS`, so the CI job above cannot silently run zero cases.

### One of the 13 was a detector FALSE POSITIVE, and saying so matters

`acp_engine_turn` is **not** an all-ignored binary. `#[path = "support/mod.rs"] mod support;`
compiles **8 further non-ignored tests** into the same binary — measured as `8 filtered out` under
`--ignored`. So `cargo test --test acp_engine_turn` prints `8 passed` and exits 0 while executing
**neither** of the two cases the binary is named for.

That is *worse* than a plain zero-execution suite: the program's own rule — "read the `N passed`
count back" — sees a healthy `8 passed` and is satisfied. Its guard is worded for that specific
hazard rather than copied.

The detector now resolves `mod` declarations and `#[path]` includes and classifies the whole
binary, because it had reported this file as flavour (a) on a single-file scan. **A detector that
over-reports is as useless as one that under-reports — it trains the reader to skim the list.**

### Flavour (d) — 19 instances, not 2

`kr07-zero-test-inventory.py` short-circuited on `total == 0`, which is exactly why a file-level
`#![cfg(...)]` was invisible to it. The gate is now checked *before* that short-circuit, and
feature gates are bucketed apart from platform gates (a feature gate blanks a binary based on how
cargo was invoked, on a host that could otherwise have run it).

**Measured: 19 feature-gated and 25 platform-gated test binaries**, against a prior estimate of
two. The largest is `wcore-mcp/tests/mcp_integration.rs`, which blanks **16 tests** without
`--features test-utils` while printing `running 0 tests` and exiting 0. Others include
`packaged_driver_gate` (4), `f23a_boundary_drive` (3), `docker_smoke` (5),
`harness_failure_injection` (2), `embedder_openai_live` (3).

### The fix for (d) is at the invocation site, not file by file

`.config/nextest.toml` had **no `no-tests` policy** and `vx.toml` pins nextest unversioned, so the
behaviour depended on whichever CLI happened to be installed. Set explicitly:

```toml
[profile.default]
no-tests = "fail"
```

Inherited by profiles `ci` / `e2e` / `eval`. Scoped to the whole invocation, not per binary, so a
workspace run is unaffected — only a targeted run matching nothing fails. This closes flavours
(c) and (d) generically instead of restructuring 19 files.

**Falsified on hetzner, known-positive against known-negative:**

| Case | Command | Result |
|---|---|---|
| known-POSITIVE (zero match) | `cargo nextest run -p wcore-observability --test otlp_local_test` | `Starting 0 tests across 1 binary` → `error: no tests to run` **rc=4** |
| known-NEGATIVE (normal run) | `cargo nextest run -p wcore-observability --lib` | `50 tests run: 50 passed, 0 skipped` **rc=0** |

Note this is a nextest-only guarantee. Plain `cargo test` retains the hazard, which is why the
detector still exists.

### The detector's own known-positive / known-negative proof

`--self-test` writes 4 known-positive and 4 known-negative fixtures and asserts the detector
**separates** them:

```
PASS  positive_feature_gate / positive_platform_gate / positive_compound_gate
PASS  positive_gate_below_doc_comment
PASS  negative_no_gate / negative_prose_only
PASS  negative_item_level_cfg / negative_indented_inner_attr
4 known-positive, 4 known-negative, 0 mismatched
SELF-TEST PASSED       rc=0
```

**And the self-test can fail.** A detector nobody falsifies is the defect it hunts, so I
reintroduced this script's ancestral defect — matching a cfg gate mentioned in doc-comment
**prose** — into a copy and re-ran:

```
FAIL  negative_prose_only    flagged=True  expected flagged=False
4 known-positive, 4 known-negative, 1 mismatched
SELF-TEST FAILED: the detector does not separate positives from negatives    rc=1
```

Mutant rc=1, real rc=0. The separation is measured, not claimed.

---

## Still open — stated rather than closed

- **No macOS result was obtained.** Runs 30362524522 / 30363073170 were still `queued` when this
  was written. The workflow and its gate are proven-correct by construction and review, **not by
  a green run.** This is the one target I did not complete.
- **`acp_engine_turn` remains red** (F-28C-02 / F-28C-03), by design — reporting it red was
  correct, engineering a green was not.
- **`tool_token_bench_smoke` remains red** (F-28C-01). The diagnostics were fixed; the missing
  sandbox wiring was not.
- **`actor_acl_test` remains red under `--ignored`** and should stay that way until the sub-agent
  ACL path is wired.
- **Flavour (d) is closed at the invocation site for nextest only**; the 19 gated binaries are
  unchanged, and `cargo test` can still run zero of them silently.
- `desktop_contract_corpus` not run — `CLASS-CONTRACT-01`, structural, not mine.
- The predecessor's F-KR-09 (O(objects) ACL grant cost) is untouched and still MEDIUM.

## State left behind

hetzner worktree `/root/wayland-28cleanup` (branch `hz/28-cleanup`) and logs `/root/28c-*.log`,
retained for re-reading. No Windows host was used by this lane, so the five `wlKR07*` scheduled
tasks on `seandesktop` were **left exactly as found** — not read, not cleaned, not touched — and
neither was `C:\ferrox-win-23B04` or `/root/wayland-23B-04`. No other lane's state was modified.
Every figure above is from a serial run and is labelled with the host it came from.
