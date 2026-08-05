# NOTES — lane/fix-headless-keyring

Base integration commit: `bc90ee1c1f08b76e6682b4beab2386fc7216a52e`.
Started 2026-07-30.

## Minute 0-15: source located, single upstream point identified (pre-build, unproven)

Two UAT lanes reported the same user-visible failure from opposite directions:

- `uat-channels-live` #1: every inbound channel turn → `WARN inbound turn dispatch failed
  error=Session persistence authority unavailable: secure recovery storage is unavailable:
  no OS keyring was usable and no encrypted credentials vault is unlocked`.
  Workaround found: `[session] enabled = false`.
- `uat-tui-unix` F2: `wayland-core --no-tui "<prompt>"` → rc=1, same message.
  Workaround found: `WAYLAND_VAULT_PASSPHRASE`.

### The two workarounds are two ends of ONE call chain

```
AgentEngine::run / run_with_content            crates/wcore-agent/src/engine.rs:6258
  └─ if self.session_journal.is_some()          <- [session] enabled=false kills this arm
       self.recovery_request_protection.preflight(&self.config)?      engine.rs:6259
         └─ RecoveryRequestProtector::with_key   recovery_confidential.rs:192
              ├─ reject_backend_without_confidential_storage(config)  (static half)
              └─ config.open_confidential_credentials_store()         recovery_confidential.rs:207
                   └─ wcore_config::credentials::open_confidential_store  credentials.rs:1451
                        └─ select_confidential_backend                credentials.rs:470
                             Auto => keyring available? else vault_unlock_material_present()?
                                     else Err("no confidential credential backend is available")
                                                                       <- WAYLAND_VAULT_PASSPHRASE
                                                                          flips this to Ok
```

`[session] enabled = false` removes the *caller*. `WAYLAND_VAULT_PASSPHRASE` satisfies the
*callee*. One chain, two ends — which is why two lanes found two different fixes.

### The defect is TIMING, and half of it was already fixed

`init_session` (engine.rs:3608-3646) already refuses early — but only for the **statically
decidable** half:

```rust
// engine.rs:3620
crate::recovery_confidential::reject_backend_without_confidential_storage(&self.config)
```

and its doc comment (recovery_confidential.rs:107-113) states the intent exactly:

> "this is a pure function of config with no side effects, so a persisted session can refuse
> to open instead of accepting the session and failing every turn afterwards."

The **dynamically decidable** half — "no OS keyring and no unlocked vault", i.e. *the entire
headless-Linux case* — is NOT checked there. It is only discovered at the first turn.
`preflight(&self.config)` takes no turn state; it is a pure function of config + ambient env,
so it can run at exactly the place the static check already lives.

**Single upstream point: `AgentEngine::init_session`, `crates/wcore-agent/src/engine.rs:3614`
(the `if let Some(mgr) = &self.session_manager` arm).** Both entrances pass through it —
the channel path must, because `session_journal` (the thing gating the per-turn preflight) is
only ever set there.

## Chosen behaviour: DEGRADE, scoped to one cause (rationale, pre-proof)

`NoSecureBackendAvailable` (no keyring + no vault) → run this session **without durable
persistence**, announce once at startup. Every other confidential error keeps today's refusal.

Why degrade rather than refuse at startup:
1. It writes **no** confidential material anywhere, so it relaxes no security property. It is
   the same posture the operator reaches today by hand with `[session] enabled = false`.
2. A gateway that answers messages without crash-recovery beats one that answers none.
3. It makes `Healthy` truthful *structurally* — the product can answer — instead of by adding
   another health probe. That keeps me out of `fix-channel-health-truth`'s files.
4. An operator on a server should not have to know an env var exists.

Why scope it to ONE cause: a wrong vault passphrase (`SecureStoreUnreadable`) or an explicitly
configured plaintext backend (`PlaintextBackendRejected`) are configurations the operator
*chose*. Silently degrading those would hide a real misconfiguration. Only "this host has no
secure store at all" degrades.

## CORRECTION — `init_session` is NOT the single point. My own first answer was wrong.

Measured after writing the section above. `session_journal` has **three** production
writers, not one:

| # | site | reached by |
|---|---|---|
| 1 | `init_session` engine.rs:3643 | CLI fresh run; channel conversation where `is_new` |
| 2 | `resume_with_provider_parts` engine.rs:3388 (constructor) | `AgentBootstrap::resume` — CLI `--resume`, and **every channel conversation that already exists on disk** (`channel_dispatch.rs:227` `load_for_run_if_exists`) |
| 3 | `switch_active_session` engine.rs:3712 | in-TUI session switch |

Site 2 bypasses `init_session` entirely, so an `init_session`-only fix would have left
every *restarted* channel conversation broken while looking green on a fresh one. This also
means the UAT lane's `[session] enabled = false` workaround is **itself incomplete**: with
sessions disabled, `channel_dispatch` still builds its own `SessionManager` and still hands
`bootstrap.resume()` a live journal, so a resumed conversation keeps a journal and keeps
failing. Their conversations were all new, so they never saw it.

**The real single point is one layer up: `session.enabled` itself.** It is read in exactly
two places (`engine.rs:3094`, `engine.rs:3336`) and it is the switch the working workaround
flips. Resolving it correctly at `Config::resolve` — one site, upstream of every engine,
every entrance, and every surface — covers all three journal writers at once.

Chosen implementation:
1. `Config::resolve_inner_from_files` (config.rs ~2459): if durable sessions are on and this
   host has no confidential-capable store, set `session.enabled = false` and announce once.
2. `resume_with_provider_parts`: an engine whose sessions are disabled must not hold a
   journal. Closes the `enabled = false` + resume hole above (site 2).

## Quadrant 2 — BUG REPRODUCED BEFORE THE FIX (hetzner, pre-fix binary)

`wayland-core --build-info` → `wayland-core 0.12.25 (source bc90ee1c1f08b76e6682b4beab2386fc7216a52e)`
sha256 `05116fee539dc04533c312a4f3c9ce18bd711cbec60ba6c40b270c842e8f418d`
Status read back from a file by a separate ssh call, never from `$?` across the pipe.

| run | vault | WLRC | answer | session files |
|---|---|---|---|---|
| `q2-prefix-novault` | no | **1** | none | **0** |
| `q3-prefix-vault` | yes | **0** | `* WLHK_TURN_OK` | **2** |

`q2` stderr, verbatim:
```
error: Session persistence authority unavailable: secure recovery storage is unavailable:
no OS keyring was usable and no encrypted credentials vault is unlocked. ...
```

The pair is the instrument control: the harness discriminates in both directions on the
same binary, same host, same prompt — so the `rc=1` is the defect and not a dead harness.
Sharp detail: `q2` **did** create `sessions/<id>.journal` before dying. The session opens;
only the turn fails. That is the "accept the work, then fail invisibly" shape exactly.

## Checklist — all closed

- [x] Reproduce the bug on hetzner BEFORE the fix (quadrant 2).
- [x] Implement (4 files; see below).
- [x] Quadrants 1/3/4 on hetzner + the `gateway run` surface.
- [x] fmt / check --workspace --all-targets / cargo metadata --locked / clippy.

---

# OUTCOME

**Fixed and live-proven on a genuinely keyring-less host.** Final SHA
`551d900127c03989061b44f170674c12419f6971`. Decision rationale, the cross-audit
panel and the case against my own choice: `DECISION.md`.

## Files changed (4)

| file | change |
|---|---|
| `wcore-config/src/credentials.rs` | extract `confidential_backend_plan` (opener and probe cannot drift); add `confidential_backend_available` — a **read-only** twin of `open_confidential_store`: pins nothing, locks nothing, migrates nothing |
| `wcore-config/src/config.rs` | `Config::confidential_recovery_storage_available()`; pure `durable_sessions_must_be_disabled` predicate; the startup notice; `durable_sessions_disabled_by_host()` reporting seam; call site in `resolve_inner_from_files` |
| `wcore-agent/src/engine.rs` | `resume_with_provider_parts` drops an incoming journal when sessions are off (closes journal writer #2); test-only `has_durable_journal()` |
| `wcore-agent/tests/headless_durable_session_test.rs` | new — the resume invariant plus its control |

Neither fenced file (`wcore-cli/src/lib.rs`, `wcore-cli/src/main.rs`) is touched.

## Four quadrants

Each row's binary self-reports its own source SHA via `--build-info`. Exit codes
written to a file on hetzner and read back by a **separate** ssh call.

| # | run | vault | rc | notice | turn OK | old error | session files | source |
|---|---|---|---|---|---|---|---|---|
| **2** | `q2-prefix-novault` | no | **1** | 0 | 0 | **1** | 0 | `bc90ee1c` |
| — | `q3-prefix-vault` | yes | 0 | 0 | 1 | 0 | 2 | `bc90ee1c` |
| **1** | `f1-final-novault` | no | **0** | **1** | **1** | 0 | 0 | `551d9001` |
| **3** | `f3-final-vault` | yes | **0** | **0** | **1** | 0 | **2** | `551d9001` |

Quadrant 4 both ways: the notice appears in **1 of 4** runs and in none of the
other three, including the post-fix vault run (the must-not-appear arm).
Both instrument controls present in all four captures: a known-positive in every
log (2 hits each) and a known-negative in none.

Fifth run — `gateway run` on a keyring-less host: the notice prints **before the
gateway's own first line**, the gateway starts and drains clean.

## Gates at `551d9001` (hetzner)

`cargo metadata --locked` 0 · `fmt --check` 0 · `check --workspace --all-targets` 0
`test -p wcore-config --lib` **579 passed / 0 failed / 0 ignored / 0 filtered**
`test -p wcore-agent` (2 targets) **2 + 3 passed**, 0 ignored, 0 filtered
`test -p wcore-cli --test json_stream_startup_refusal` **6 passed**
clippy scoped to all authored code: **0, 0, 0**
clippy `--workspace --all-targets -D warnings`: **101 — PRE-EXISTING, proven**
(three files byte-identical at base; see `clippy-preexisting-proof.txt`), plus a
negative control showing `-D warnings` is armed.

## Not done / not measurable

1. **Channel inbound not driven live** — no platform credential in this lane.
2. **`switch_active_session` (journal writer #3) still unguarded** — residual.
3. **`--json-stream`/Desktop hosts are not told** — the notice is stderr and the
   decision precedes any protocol writer.
4. **Keyring-PRESENT arm proven via the vault, not a real OS keyring** — same
   branch of `select_confidential_backend`; a real macOS Keychain run is UNRUN.
5. **macOS at this SHA unrun entirely.**

## Post-summary addendum: `wcore-agent --lib` (measured, not assumed)

My change touches `engine.rs`, so the crate's own lib suite had to be run. Under
parallel execution on hetzner it reds with ~16 "session journal writer lease is
already held" failures. Four measurements show that is **pre-existing flakiness
under host contention, not this lane** (`agent-lib-suite-flakiness.txt`):

1. My SHA, **single-threaded**: `2252 passed; 0 failed; 3 ignored; 0 filtered`.
2. **Same command, same commit, twice**: 17 failed both times — but **6 of the 17
   are different tests** between the two runs. A regression cannot do that.
3. **Integration base**, same parallel command: already `14 failed`.
4. The base and head failure sets **overlap but neither contains the other** —
   base fails six tests my SHA passes.

Reported red, not silenced: no `#[ignore]`, no retry, no `serial_test` guard added.
