# F-KR-07 disposed FIXED, and the suites that certified nothing

**Branch:** `lane/kr07-suites`, off `plan/f20-unified-audit-repair` at merge-base `5d5164d8`.
**Hosts:** `SeanD@seandesktop` (Windows, `C:\wl-kr07`), `hetzner-dsm` (Linux, `/root/wayland-kr07`).
**Never merged into the integration branch. No PR, no tag, no issue closed. `wcore-contract generate` NOT run.**

Every live-Windows figure below is from a **serial** (`--test-threads=1`) run; each is
labelled with the run it came from. Evidence: `evidence/28-kr07-suites/`.

---

## F-KR-07 — **FIXED**. It was real, it was a test-construction fault, and I watched it fail.

### The criterion, and therefore the available dispositions

Phase 28 Success Criterion 1, verbatim from `.planning/ROADMAP.md`:

> 1. Native macOS, Linux, and Windows pass the required hostile platform matrix with no skipped critical case.

`live_cmd_runs_when_allowlist_has_missing_path` is a native Windows AppContainer
acceptance case in that matrix, so a deterministic red there contradicts Criterion 1.
**Amendment A2 therefore applies and closes accept and defer** — only FIXED or DISPROVED
were available. This lane returns **FIXED**.

### The ladder, one property per rung

Twelve rungs across three runs. The failing test's already-passing sibling is byte-identical
apart from `fs_read_allow`, so the ladder split that field apart.

| Rung | Property isolated | Result |
|---|---|---|
| 1 | no allowlist at all | **112 ms, green** — sandbox and command are fine |
| 3 | the ABSENT path **alone** — the property the test is NAMED for | **107 ms, green** |
| 5 / 12 | small real dir **+** absent path — the two-entry shape | **99 ms / 133 ms, green** |
| 2 | `%TEMP%` + absent, first touch | 21,499 ms, green — **86% of its own ceiling** |
| 4, 6, 7 | `%TEMP%`, subsequent touches | 10,164 / 9,285 / 9,084 ms |
| 8 | cost vs. subtree size, 0 / 500 / 2 000 / 8 000 objects | 95 / 228 / 299 / **1,610 ms** |
| 11 vs 12 | 200,000 objects vs 200, everything else identical | **19,487 ms vs 133 ms — 146x** |

Rungs 3 and 5 clear the absent path in ~0.1 s, so **the red was never about the missing
allowlist entry the finding is named for.** The cost is the *other* entry: the test granted
over `std::env::temp_dir()`, which is unbounded, shared with every process on the host, and
outside the test's control. `apply_explicit_access` sets `SUB_CONTAINERS_AND_OBJECTS_INHERIT`,
so `SetNamedSecurityInfoW` propagates across the whole subtree, and `cleanup_locked` calls
`revoke_intents`, so the walk is paid **on every execution**, not once.

### The rung that failed, and why it is left failing

Rung 11 predicted that 200,000 objects would exhaust the 25 s budget (`manifest.timeout` 10 s
plus the 15 s setup grace `windows_impl::process` adds, because the inner `WaitForSingleObject`
bounds only the child's *run*). It reached 19,487 ms and returned `Ok(0)`. **The rung failed on
its own assertion — `PREDICTION REFUTED` — and I left it in the tree that way.** Cost is
sublinear at scale (0.20 ms/object at 8,000, 0.097 ms at 200,000), so my arithmetic was wrong.
A rung edited until it passes measures nothing.

### The watched red

Run 1 of the re-measurement rung (19:45) returned **5/5 PASS**, worst 10,629 ms. Had I stopped
there I would have filed "cannot reproduce" and been wrong. Fifteen minutes later, on the same
host at the same commit, the **same rung — still granting over `%TEMP%` —** returned:

```
KR07B_REMEASURE run=1 verdict=FAIL Timeout elapsed_ms=25003
KR07B_REMEASURE_SUMMARY passes=2 fails=3 worst_ms=25009 pct_of_25s_ceiling=100.0
```

**`SandboxError::Timeout` at exactly the ceiling — the reported failure mode, reproduced.**
The original 12/12 report was correct; my first green was a lucky window. In that *same
invocation*, the repaired test passed:

```
test live_cmd_runs_when_allowlist_has_missing_path ... ok
test result: ok. 5 passed; 0 failed
```

Unrepaired shape red and repaired test green **in one run** is the matched pair the fix rests on.

### What was changed, and what was not

The real allowlist entry is now a directory the test **owns**. Every assertion is unchanged —
still exit 0, still the `allowlist-skip-ok` stdout marker, still one real path beside one absent
path. **No timeout was raised, nothing was `#[ignore]`d, no assertion relaxed.** The repair
lands on a test that was *green* at the time, so it is a determinism repair, not a reach for green.

**This is a test fix, not a product fix — stated plainly.** The product's O(objects) grant cost
is unchanged and is filed below.

### New finding opened

| ID | Severity | Finding |
|---|---|---|
| **F-KR-09** | **MEDIUM** → BACKLOG | AppContainer ACL grant+revoke is O(objects under the granted path) and is paid on **every** execution: 133 ms at 200 objects, ~10 s at `%TEMP%`'s 57,636, 19,487 ms at 200,000. The field allowlist this very test documents (`~/.cache`, `~/.cargo`, `~/.npm`, `~/.rustup`) is exactly the large-tree case, so a real user can pay tens of seconds of setup on every sandboxed command. Not a containment defect; a cost one. MEDIUM per standing policy, non-blocking. |

---

## Target 2 — the suites that report `ok` having run zero tests

### The inventory rots, so I re-derived it

Independent detector added at `.planning/scripts/kr07-zero-test-inventory.py`; it never collects
a comment line into an attribute block, because the inherited generator once read doc-comment
**prose** as an ignore attribute. It found **16** all-ignored binaries, not 15. The extra one is
`crates/wcore-sandbox/tests/live_cwd_verbatim.rs` (3/3), added by `a870ba8b` *after* the inherited
inventory was written. **13** suites with only *some* tests ignored were counted separately and
**left alone**, per the distinction I was told to preserve.

### The sharpest finding: three "zero-execution guards" that were themselves `#[ignore]`d

`live_fs_acl`, `hard_process_containment_windows` and `live_cwd_verbatim` each contained a test
*named* a zero-execution guard and marked `#[ignore]`. It could only fire under `--ignored` — by
which point the real cases were running and nothing needed guarding. **Inert against precisely the
scenario it existed for.** Fixed by making each always run, with the env var as a *condition*
rather than an assertion (the assertion is what forced the `#[ignore]`). Falsifiability measured,
not asserted (run 4):

```
env SET,   no --ignored -> test result: FAILED. 0 passed; 1 failed; 11 ignored   rc=101
env UNSET, no --ignored -> test result: ok.     1 passed; 0 failed; 11 ignored   rc=0
```

Before the change the first line read `ok. 0 passed; 12 ignored`.

### A fourth flavour, found only by running them

Beyond (a) all-ignored, (b) env-gated early `return` and (c) a filter matching nothing, there is
**(d) a file-level `#![cfg(feature = "...")]`**. Without the feature the binary contains **zero
tests** and `cargo test --test X` prints `running 0 tests` / `test result: ok` and exits 0. It is
invisible to the attribute list, to the env-gate scan and to the filter sweep. Two suites are in
this state. It needs its own fix (assert the feature at the invocation site); no fix is claimed here.

### Per-suite conversion table — every red reported

**Linux, `hetzner-dsm`, serial, `-- --ignored`:**

| Suite | Executed | Verdict |
|---|---|---|
| `wcore-agent/actor_acl_test` | 5 | **RED — 4 passed / 1 FAILED**: `sub_agent_with_deny_policy_short_circuits` |
| `wcore-agent/tool_token_bench_smoke` | 1 | **RED — 0 passed / 1 FAILED** (63.29 s) |
| `wcore-cli/acp_engine_turn` | 2 | **RED — 0 passed / 2 FAILED**: `a2a_on_message_routes_task_to_engine`, `acp_turn_streams_text_then_done` |
| `wcore-eval/acceptance_gate` | 1 | green (needs `--features acceptance-gate`) |
| `wcore-exec-backend/live_equivalence` | 1 | green, 1.57 s |
| `wcore-memory/hybrid_retriever_perf_test` | 2 | green, 9.38 s |
| `wcore-eval-scenarios/pty_tui_smoke` | 1 | green, 8.74 s |
| `wcore-eval-scenarios/cross_session_live` | 1 | **FLAVOUR B — `1 passed` in 0.00 s**, env-gated early return, no `DEEPSEEK_API_KEY` |
| `wcore-eval-scenarios/live_personas` | 1 | **FLAVOUR B — `1 passed` in 0.00 s**, same |
| `wcore-observability/otlp_local_test` | **0** | **FLAVOUR D — `running 0 tests`, `ok`, rc 0** (`#![cfg(feature = "otlp")]`) |
| `wcore-memory/bge_local_real` | **0** | **FLAVOUR D — `running 0 tests`, `ok`, rc 0** (`#![cfg(feature = "bge-local")]`) |

**Windows, `SeanDesktop`, serial, `-- --ignored` (run 4) — first execution these have ever had:**

| Suite | Executed | Verdict |
|---|---|---|
| `live_fs_acl` | 11 | **11 passed / 0 failed**, 13.81 s |
| `hard_process_containment_windows` | 5 | **5 passed / 0 failed**, 19.85 s |
| `live_cwd_verbatim` | 2 | **2 passed / 0 failed**, 1.40 s |

**Five real reds surfaced. None was fixed quietly, none was re-`#[ignore]`d, none re-gated.**
The two Flavour-B suites are *confirmed on hardware* — the inherited inventory listed them as
unchecked candidates.

### Legitimately not run, per suite

| Suite | Why |
|---|---|
| `wcore-sandbox/hard_process_containment_macos` | **No macOS build host in this lane's reach.** Cargo is forbidden on the Mac; hetzner is Linux, seandesktop is Windows. Not a skip — an absent machine. |
| `wcore-sandbox/live_integrity_macos` | Same. |
| `wcore-eval-scenarios/cross_session_live`, `live_personas` | Need a real `DEEPSEEK_API_KEY` and cost money. I will not supply a credential. Their *defect* (affirmative green for zero work) is independent of the key and is reported above. |
| `wcore-memory/bge_local_real` | Downloads a ~133 MB model and needs `--features bge-local`. |
| `wcore-observability/otlp_local_test` | Needs `--features otlp` and a running OTLP collector. |

### Flavour C sweep — an honest negative

Every filtered `cargo test` / `nextest run` in `justfile`, `scripts/**` and `.github/workflows/*`
was extracted and each filter checked against the real test-function names. **No stale filter
found** — `anthropic`, `openai`, `memory`, `compact` and
`acceptance_gate_meets_precision_recall_threshold` all still match. One related MEDIUM:
`.config/nextest.toml` sets **no `no-tests` policy** and `vx.toml` pins nextest unversioned
(`nextest = "cargo nextest"`), so whether a zero-match nextest run fails depends on the CLI
default of whatever version is installed. → BACKLOG.

---

## Still open — stated rather than closed

- **11 of 16 Flavour-A binaries still have no always-running guard.** I guarded the three in the
  Phase-28 native matrix and my three ladder files. Claiming 16 would be false.
- **Flavour D has no fix**, only a diagnosis and two instances.
- **The five reds are reported, not repaired** — out of this lane's brief, and repairing them
  quietly is what I was told not to do.
- **macOS legs unexecuted**, for want of a machine.
- `desktop_contract_corpus` not run — `CLASS-CONTRACT-01`, structural, not mine.

## State left behind

Four scheduled tasks registered by this lane (`wlKR07Build`, `wlKR07Ladder`, `wlKR07Run2`,
`wlKR07Run3`, `wlKR07Run4`) — **left registered, not deleted, so the orchestrator can re-read
them; no other lane's task was touched or killed.** Windows tree `C:\wl-kr07`; scratch
`C:\wl-kr07-scratch` removed. hetzner worktree `/root/wayland-kr07`. `C:\ferrox-win-23B04` and
`/root/wayland-23B-04` never touched. Neither shared-fence file (`crates/wcore-cli/src/{lib,main}.rs`)
was modified — verified against the captured merge-base `5d5164d8`, never against the branch name.
