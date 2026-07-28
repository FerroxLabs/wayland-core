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

## Harness rules I am binding myself to in this lane (§6b-ii)

- Byte-count every capture (`wc -c`), never infer emptiness from a visual blank.
- Never read `${PIPESTATUS[0]}` after a pipeline via `echo` — returns empty here.
- Read back the `N passed` count from every cargo run; never trust exit status.
- Any defect found in this harness gets fixed in THIS lane, with a 3-assertion self-test
  whose third assertion is that the old shape would have missed it.
