# NOTES — lane/json-stream-refusal (running log, appended after every measurement)

Base `plan/f20-unified-audit-repair` @ `0b5182ef`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-json-stream-refusal`.

## T+15min — static map of the startup path, before any execution

Read `crates/wcore-cli/src/main.rs` (7766 lines) and `crates/wcore-agent/src/engine.rs`.

**There are exactly TWO error-frame emit sites on the whole startup path**, both added by
issue #186:

| # | site | covers |
|---|---|---|
| E1 | `main.rs:1789-1802` | `Config::resolve_with_provenance` returned Err |
| E2 | `main.rs:4306` (`output.emit_error`) | `bootstrap.build()` returned Err |

Both construct `ProtocolEvent::Error { msg_id, error: ErrorInfo { code, message, retryable } }`.
That is the frame that already exists — the lane must reuse it, not invent one.

**The plaintext-credentials refusal named in the brief falls BETWEEN them.** Chain:

```
main.rs:4345   engine.init_session(&provider_name, cwd, session_id.as_deref())?   <-- bare `?`
engine.rs:3562 recovery_confidential::reject_backend_without_confidential_storage(&self.config)
```

`init_session` runs **after** E2 (bootstrap.build, line 4302) and **before**
`emit_ready_with_plugins_and_policy` (line 4365). So the refusal returns `Err` out of
`run_json_stream_mode`, up through `run()`, and is printed by anyhow to **stderr**. No `ready`,
no `error`, zero frames. This is the defect, and it is a bare `?`, not a missing branch.

## Candidate refusal paths with NO frame — to be enumerated exhaustively next

Inside `run_json_stream_mode`, after E2 and before `ready`:
- `4298` `session_mgr.load_for_run(resume_id)?` (resume of a bad/missing session)
- `4314`/`4319` `audit_unix_time_millis()?`
- `4345` `engine.init_session(...)?`  <-- the measured case

In `run()`, before `run_json_stream_mode` is ever called:
- `1656` `anyhow::bail!(msg)` from `json_stream_profile_guard` (3A/D3 profile fail-closed)
- `1732` `return Err(e)` for `ConfigLoadError::ParseFailed` — **this one is inside the E1
  branch but returns EARLY, above the E1 emit at 1789.** A corrupt config.toml under
  `--json-stream` therefore emits nothing even though #186 believed it covered config failures.
  This is a gap in an already-"fixed" path and is worth calling out separately.
- `1846` `resolve_resume(...)?`
- `1847` `resolve_local_execution(...)?`
- `1660`/`1840` `std::env::current_dir()?`

## Still to establish

1. Reproduce live: byte-count the stdout of a real `--json-stream` invocation in the
   plaintext-credentials condition. Expect 0 bytes / 0 frames.
2. Positive control in the SAME run: a healthy start that DOES emit `ready`. Without it a
   refusal is indistinguishable from a broken invocation — the previous probe of this area
   produced a false HIGH exactly this way.
3. Whether `--json-stream` + `init_session` failure is reachable on macOS or needs hetzner.
4. Whether other startup refusals (list above) can be covered by one chokepoint or need
   per-site emits.

## T+60min — LIVE BASELINE MEASURED (pre-fix), hetzner-dsm, debug build, BUILDRC=0

Binary built from `c731ce5b` on `hetzner-dsm`. Harness
`.planning/evidence/json-stream-refusal/run-cases.sh`, captures under `/tmp/jsr-prefix/`.
Instrument self-test PASSED (3/3) at the top of the same run.

| case | condition | rc | stdout bytes | frames | verdict |
|---|---|---|---|---|---|
| **P_OK** | session off, plaintext creds | 0 | **4480** | **27** (incl. 1 `ready`) | positive control — harness proven |
| **N_REFUSE** | session ON, plaintext creds | 1 | **0** | **0** | **DEFECT — zero frames** |
| **D_PARSE** | corrupt `config.toml` | 1 | **0** | **0** | **DEFECT** |
| **D_NOKEY** | no API key | 1 | 496 | **1** `error`/`init_failed` | already covered by #186 |
| **D_PROFILE** | `--profile` w/o `WAYLAND_HOME` | 1 | **0** | **0** | **DEFECT** |

P_OK and N_REFUSE differ in exactly ONE config key (`session.enabled`), and P_OK emits 27
frames in the same run — so N_REFUSE's zero is the product's behaviour, not a broken
invocation. That is the specific trap that produced the earlier false HIGH on this area.

N_REFUSE stdout is 0 bytes confirmed by `wc -c` AND an empty `xxd`. Its reason is the LAST
line of a 6015-byte **stderr**:

```
Error: storage.credentials.backend is set to "plaintext", which cannot hold the confidential
key that durable session recovery requires. ...
```

**Three of four refusal doors emit nothing; one of four emits.**

Two facts this baseline settles that the static read could not:

1. **D_NOKEY proves the emit mechanism and the output pump both work** — 496 bytes reach
   stdout even though the process exits immediately afterwards. So a chokepoint emit will
   also arrive; no flush redesign is needed (an explicit `flush_bounded` is still added as
   belt-and-braces).
2. **D_PARSE confirms the statically-predicted gap INSIDE #186's own fix.** `ConfigLoadError`
   returns at `main.rs:1732`, above the emit at `1789`. #186 believed it covered config
   failure; it covers the credential half and not the corrupt-file half.

## T+2h — INSTRUMENT DEFECT IN MY OWN FENCE GATE (§6b-ii), found and repaired here

Writing the §6 shared-file fence check as `git diff "$BASE" -- <files> | grep -c '^-[^-]'`
reported **0 removed lines while `--numstat` reported 3**. `git diff` issued as a direct agent
tool call is proxied by a token-optimising wrapper (rtk) that re-renders the diff **indented by
two spaces**, so `^-` matches nothing. Measured: 3849 bytes of diff, 3 real deletions, scored 0.
A fence gate that cannot see a removal cannot fail.

Repaired in `fence-gate.sh` by abandoning diff-TEXT parsing for `--numstat` (machine-readable
columns, no prefix to mangle). Loosening the regex to allow leading whitespace was **rejected**:
a diff CONTEXT line beginning with `-` (a markdown bullet) matches that too, trading a false
negative for a false positive.

**The 3-assertion self-test caught my own wrong model on its first run.** A3 originally asserted
"the wrapper is always active"; it FAILED, because the wrapper intercepts direct tool calls but
NOT git invoked inside a script. Corrected A3 now applies the wrapper's exact transform
explicitly and asserts the old matcher goes blind under it while `--numstat` does not.
That failure is the self-test doing its job — it refused to certify a repair on a false premise.

Gate proven able to fail: injecting an undeclared removal (`pub mod sandbox_cmd;`) produced
`FENCE=FAIL 1 undeclared removal(s)`; restoring it produced `FENCE=PASS`.

## T+3h — FIX LANDED, RED→GREEN, LIVE RE-PROVEN

Fix commit `743e52bb`. Chokepoint `crates/wcore-cli/src/startup_error.rs` fires at process exit
for any error escaping `run()` before `ready`. Reuses `ProtocolEvent::Error` / `msg_id: None` /
code `init_failed` — no new frame, `ready` untouched.

**RED→GREEN on the same test file, executed counts read back (never exit status):**

- base `0b5182ef`, fix absent, test present: `test result: FAILED. 2 passed; 4 failed`
- fix `743e52bb`: `test result: ok. 6 passed; 0 failed; 0 ignored; 0 filtered out`

The 2 that pass at base are exactly the 2 that should: the positive control, and the
missing-API-key path that #186 had already covered. The gate fails for the right reasons.

**Live consumer-side sweep, before vs after (same harness, same host):**

| case | pre-fix | post-fix |
|---|---|---|
| P_OK (control) | rc=0 4480B **27 frames** | rc=0 4480B **27 frames — unchanged** |
| N_REFUSE | rc=1 **0B 0 frames** | rc=1 485B **1 error frame** naming `plaintext` |
| D_PARSE | rc=1 **0B 0 frames** | rc=1 368B **1 error frame** naming the TOML error |
| D_NOKEY | rc=1 496B 1 frame | rc=1 496B **1 frame — unchanged, no duplicate** |
| D_PROFILE | rc=1 **0B 0 frames** | rc=1 490B **1 error frame** naming `WAYLAND_HOME` |

P_OK byte-identical at 4480B/27 frames excludes the "manufacture a green by making nothing
start" failure. D_NOKEY byte-identical at 496B proves the chokepoint does not double-report.

**Regression:** `wcore-protocol --test golden_v0_1_21` **22 passed; 0 failed** (the `ready` and
`error` wire shapes are unchanged); `wcore-cli --lib` serial **1837 passed; 0 failed; 1 ignored**;
the 6 `startup_error::tests::*` executed by name (`6 passed; 1832 filtered out`);
`cargo clippy -p wcore-cli --all-targets` clean; `cargo fmt --all -- --check` rc=0.

## Startup refusal paths NOT covered — stated plainly

1. **A panic during startup.** The chokepoint sees `Err`; a panic unwinds past it, so the host
   still gets nothing. `crash_sentinel` records it for the NEXT run, which does not help the
   host now. This is a crash, not a refusal, and closing it needs a panic hook — a bigger
   change than this lane should make.
2. **Failures before `Cli::parse()`** — `activate_for_launch()` and `load_wayland_env_file()`
   run before protocol mode is even known, so nothing can be attributed to a host yet.
3. **Clap argument-parse errors**, which clap prints and exits on directly.
4. **Post-`ready` session failures** — a deliberate scope boundary, not an oversight: past
   `ready` the session is live and the protocol sink owns error reporting.
5. **SIGTERM during startup** returns `Ok(SUCCESS)` through the shutdown path and emits nothing.

## Harness rules I am binding myself to in this lane (§6b-ii)

- Byte-count every capture (`wc -c`), never infer emptiness from a visual blank.
- Never read `${PIPESTATUS[0]}` after a pipeline via `echo` — returns empty here.
- Read back the `N passed` count from every cargo run; never trust exit status.
- Any defect found in this harness gets fixed in THIS lane, with a 3-assertion self-test
  whose third assertion is that the old shape would have missed it.
