# FINDING — a `--json-stream` startup refusal told the host nothing at all

**Lane** `lane/json-stream-refusal` · **base** `plan/f20-unified-audit-repair` @ `0b5182ef`
**Severity** **HIGH** · **Status** reproduced, fixed, RED→GREEN, live re-proven from the
consumer's side
**Instrument** `wayland-core 0.12.25`, debug, built on `hetzner-dsm` from this branch, `BUILDRC=0`

---

## 1. The defect

Over `--json-stream` the host — the Electron desktop app — reads **stdout** and nothing else.
A startup refusal returned `Err` out of `run()`, where `anyhow` printed it to **stderr**. The
host saw a pipe that opened and closed carrying **zero bytes**.

The product knew exactly what was wrong and said so, on a channel nobody was listening on.

This is the same family as the four silent-failure defects fixed in the last day, and worse,
because the consumer is our own desktop app: its user gets a spinner or a crash instead of
"your credentials backend cannot hold a session key".

---

## 2. Reproduction, with frame counts

Harness `.planning/evidence/json-stream-refusal/run-cases.sh`. Each case writes an isolated
`WAYLAND_HOME`, runs the real binary with `--json-stream`, closes stdin, and captures **stdout
and stderr separately** — merging them would destroy the very distinction under test. Frames
are counted by parsing stdout as JSON Lines (`framecount.py`), never by grepping.

**Pre-fix, base `0b5182ef`:**

| case | condition | rc | stdout | frames | verdict |
|---|---|---|---|---|---|
| **P_OK** | durable sessions off | 0 | 4480 B | **27** (incl. `ready`) | positive control |
| **N_REFUSE** | durable sessions on, `credentials.backend = "plaintext"` | 1 | **0 B** | **0** | **the defect** |
| **D_PARSE** | corrupt `config.toml` | 1 | **0 B** | **0** | **defect** |
| **D_NOKEY** | no API key | 1 | 496 B | 1 `error` | already covered by #186 |
| **D_PROFILE** | `--profile` without `WAYLAND_HOME` | 1 | **0 B** | **0** | **defect** |

**Three of four refusal doors emitted nothing.** `N_REFUSE` stdout is 0 bytes by both `wc -c`
and an empty `xxd`; its reason is the last line of a 6015-byte stderr.

### This is the defect and not the harness

A previous probe of this area produced a **false HIGH** by reproducing its own isolation bug and
reading only stdout. Two things rule that out here:

- **P_OK and N_REFUSE differ in exactly one config key** (`session.enabled`) and run in the same
  invocation of the same harness. P_OK emits 27 frames. A harness that could not observe the
  protocol could not have produced them.
- Isolation is via **`WAYLAND_HOME`**, the product's own mechanism, not `HOME`. The earlier probe
  failed because `HOME` does not isolate config on Windows (`dirs::home_dir()` reads
  `USERPROFILE`). The test sets `WAYLAND_HOME`, `HOME` **and** `USERPROFILE`.

`ready` emits, in well under a second, exactly as the earlier correction said.

---

## 3. Root cause

Two error-frame emit sites existed, both from issue #186: `main.rs:1789` (config resolution
failed) and `main.rs:4306` (`bootstrap.build()` failed). The measured refusal falls **between**
them:

```
main.rs:4345    engine.init_session(...)?                                  <- a bare `?`
engine.rs:3562  recovery_confidential::reject_backend_without_confidential_storage(&config)
```

`init_session` runs after the second emit site and before `emit_ready…` at `main.rs:4365`, so
its `Err` propagated out with no frame. `D_PARSE` is a second, independent gap **inside #186's
own fix**: `ConfigLoadError` returns at `main.rs:1732`, above the emit at `1789`.

---

## 4. The fix

`crates/wcore-cli/src/startup_error.rs` (new) — a **chokepoint at process exit**, not a fourth
patch. Any error escaping `run()` before `ready` produces one error frame. Patching the three
doors individually would leave the next `?` added to the startup path silently broken again.

- **Uses the frame that already exists.** `ProtocolEvent::Error` with `msg_id: None` — the
  established session-level shape pinned by `golden_v0_1_21.rs` — and the same `init_failed`
  code the #186 sites already emit. **No new frame, no new field, `ready` untouched.** A host
  that already handles `init_failed` handles the newly-covered refusals with no change.
- **Exactly one frame.** The two #186 sites now claim the emission, so their more specific
  messages win and the chokepoint stands down. Proven by `D_NOKEY` being byte-identical
  pre- and post-fix (496 B, 1 frame).
- **Scoped to pre-`ready`.** After `ready` the session is live and the protocol sink owns error
  reporting; the chokepoint deliberately stays silent there rather than double-reporting.

---

## 5. Coverage — what is fixed, and what is not

**Covered.** Every error returning out of `run()` before `ready`. That is structural, not
enumerated, so refusal paths nobody has listed are covered too. All four measured doors now
emit, each naming its own reason. There is no `process::exit` on this path to bypass it.

**NOT covered — stated plainly:**

1. **A panic during startup.** The chokepoint sees `Err`; a panic unwinds past it. `crash_sentinel`
   records it for the *next* run, which does not help the host now. This is a crash rather than a
   refusal and closing it needs a panic hook — a larger change than this lane should make.
2. **Failures before `Cli::parse()`** (`activate_for_launch`, `load_wayland_env_file`), which run
   before protocol mode is known.
3. **Clap argument-parse errors**, which clap prints and exits on directly.
4. **Post-`ready` session failures** — a deliberate boundary, above.
5. **SIGTERM during startup**, which returns `Ok(SUCCESS)` and emits nothing.

---

## 6. Consumer-side proof

`crates/wcore-cli/tests/json_stream_startup_refusal.rs` spawns the real binary and reads its
**stdout** as the host does. Reading from stderr would not count — that is the defect.

**RED→GREEN on the identical test file, executed counts read back, never exit status:**

```
base 0b5182ef (fix absent, test present):  test result: FAILED. 2 passed; 4 failed
fix  743e52bb:                             test result: ok. 6 passed; 0 failed; 0 ignored;
                                                            0 measured; 0 filtered out
```

The 2 passing at base are exactly the 2 that should — the positive control and the
already-covered `#186` path — so the suite fails for the right reasons and is not self-passing.
Each refusal test asserts the host can **name** the reason, not merely that bytes arrived.

**Live sweep, same harness and host, before vs after:**

| case | pre-fix | post-fix |
|---|---|---|
| P_OK (control) | rc=0 4480 B **27 frames** | rc=0 4480 B **27 frames — unchanged** |
| N_REFUSE | rc=1 **0 B / 0 frames** | rc=1 485 B **1 frame**, names `plaintext` |
| D_PARSE | rc=1 **0 B / 0 frames** | rc=1 368 B **1 frame**, names the TOML error |
| D_NOKEY | rc=1 496 B / 1 frame | rc=1 496 B **1 frame — unchanged** |
| D_PROFILE | rc=1 **0 B / 0 frames** | rc=1 490 B **1 frame**, names `WAYLAND_HOME` |

What the host now receives for the brief's case:

```json
{"type":"error","error":{"code":"init_failed","retryable":false,
 "message":"Engine failed to start: storage.credentials.backend is set to \"plaintext\",
 which cannot hold the confidential key that durable session recovery requires. ..."}}
```

**P_OK is byte-identical at 4480 B / 27 frames**, which excludes the "manufacture a green by
making nothing start" failure: had the fix broken startup, every refusal assertion would still
pass and only the control would catch it.

**Regression:** `wcore-protocol --test golden_v0_1_21` **22 passed; 0 failed** (wire shapes
unchanged) · `wcore-cli --lib` serial **1837 passed; 0 failed; 1 ignored** · the 6
`startup_error::tests::*` executed by name (`6 passed; 1832 filtered out`) ·
`cargo clippy -p wcore-cli --all-targets` clean · `cargo fmt --all -- --check` rc=0.

---

## 7. Instrument defect found and repaired in this lane (§6b-ii)

My own §6 fence gate carried the defect class it was hunting. Written as
`git diff "$BASE" -- <files> | grep -c '^-[^-]'` it reported **0 removed lines while `--numstat`
reported 3**: `git diff` issued as a direct agent tool call is proxied by a token-optimising
wrapper that re-renders the diff **indented by two spaces**, so `^-` matches nothing. Measured:
3849 bytes of diff, 3 real deletions, scored 0. Same shape as the recorded kimi vote-loss.

Repaired by abandoning diff-text parsing for `--numstat`. Loosening the regex to allow leading
whitespace was rejected — a context line beginning with `-` matches that too, trading a false
negative for a false positive.

**The 3-assertion self-test caught my own wrong model on its first run.** A3 originally asserted
the wrapper was always active; it FAILED, because the wrapper intercepts direct tool calls but
not git inside a script. Corrected A3 applies the wrapper's exact transform and asserts the old
matcher goes blind under it while `--numstat` does not. `framecount.py` carries the same
three-assertion discipline.

Gate proven able to fail: injecting an undeclared removal produced `FENCE=FAIL`; restoring it
produced `FENCE=PASS`.

---

## 8. Shared-file fence (§6) — declared, non-additive edits

The fence rule is additive-only. **This lane could not be purely additive** and says so rather
than hiding it. Three existing lines in `crates/wcore-cli/src/main.rs` were modified, each
load-bearing, none cosmetic — no reformatting, reordering, renaming or re-sorting:

| line | why it had to change |
|---|---|
| `runtime.block_on(run_until_shutdown(run(), shutdown_signal()))` | bind the result so the chokepoint can see it |
| `if cli.json_stream {` | claim the emission so the chokepoint does not double-report |
| `output.emit_error(&init_failure_message(&e, &provider_name), false);` | same, at the second #186 site |

`crates/wcore-cli/src/lib.rs` is **+8 / −0** (one `pub mod` plus its comment). `fence-gate.sh`
enforces this as an explicit allow-list: any removal *not* on the list fails the gate, so the
exemption cannot silently widen.

---

## 9. Severity: HIGH

The consumer is our own shipping desktop app; the failure is total (zero bytes, not a degraded
message); it is the default configuration on any host without a usable keyring; and the product
already computes the correct, actionable reason and discards it. Not CRITICAL: it fails closed,
no data loss, no security boundary crossed, and the reason is recoverable from stderr by an
operator with a terminal — which a desktop user does not have.

---

## 10. Reproduce

```bash
export BIN=<debug or release wayland-core>; export OUT=/tmp/jsr-out
bash .planning/evidence/json-stream-refusal/run-cases.sh
```

P_OK is the harness's own gate: if it does not show a `ready` frame, the harness is broken and
every other row is void. Transcripts for both states are committed under
`.planning/evidence/json-stream-refusal/transcripts-prefix/` and `…-postfix/`.
