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

## Harness rules I am binding myself to in this lane (§6b-ii)

- Byte-count every capture (`wc -c`), never infer emptiness from a visual blank.
- Never read `${PIPESTATUS[0]}` after a pipeline via `echo` — returns empty here.
- Read back the `N passed` count from every cargo run; never trust exit status.
- Any defect found in this harness gets fixed in THIS lane, with a 3-assertion self-test
  whose third assertion is that the old shape would have missed it.
