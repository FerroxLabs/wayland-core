# 20A — D1 and D3 repair

Lane `lane/uat-d1d3`, HEAD `7dd609e3`. Source of truth for the defects:
`20A-LIVE-WINDOWS-UAT.md` §D1 and §D3. D2 was not touched — no resume or
reconcile surface is modified by this lane.

**Verdict: both defects fixed and live-proven. One correction to the brief: D1
is not Windows-specific and it is not limited to refusals.**

---

## D1 — a refused tool call leaves a nonterminal tool execution

### What it actually was

The dispatcher models a tool execution as a lease: `Prepared → Running →
{Succeeded | Failed | NotStarted | Unknown}`. Only the first three are
terminal. `require_turn_descendants_terminal` refuses `TurnCommitted`,
`TurnFailed` and `TurnCancelled` while any descendant is `Unknown`, which is
the "invalid journal state transition" the UAT saw.

After every dispatch, `execute_single_with_streaming` did this:

```rust
if unknown_effect.is_none() && r.is_error
    && (matches!(effect_contract.kind, ToolEffectKind::Opaque) || …)
{ unknown_effect = Some((ToolUnknownReason::AmbiguousFailure { … }, …)); }
…
lease.unknown(reason, evidence).map(|_| ())   // UnknownToolEffect dropped
```

`ToolEffectKind::Opaque` is the **default** contract, so it covered every tool
that had not opted out. `lease.unknown()` returns an `UnknownToolEffect`
carrying `#[must_use = "an unknown tool effect must be reconciled or surfaced
to an operator"]`, and `.map(|_| ())` discarded it. Nothing in the process ever
resolved it; resolution existed only as a host protocol command and a
post-crash path.

**Two corrections to the brief's framing, both load-bearing:**

1. **Not Windows-specific.** Reproduced on Linux (hetzner) with the same
   malformed path, byte-identical failure. Evidence below.
2. **Not limited to refusals.** The trigger is `is_error: true` from an
   opaque-contract tool. That includes a refused path, a `Grep` on a missing
   file, and — `bash.rs` sets `is_error: exit_code != 0` — **any nonzero shell
   exit**. Every one of those killed the session. The refusal was the way the
   UAT happened to hit it, not the boundary of it.

### The enumeration the brief asked for

There is exactly **one** place in production where a running tool execution is
terminalized: the terminal-append block in `orchestration/mod.rs`. Every
refusal in every tool — `read.rs` path validation, `bash.rs` sandbox
unavailability, the bash credential denylist, `gcloud`/`kubectl`/`aws` verb
refusals, MCP and plugin adapter errors — reaches it as an ordinary
`is_error` `ToolResult`. So this is a single choke point, not five doors, and
fixing per-refusal-site would have been the wrong shape.

Three ways a lease could nonetheless survive nonterminal were found and closed:

| Path | Result before | Now |
|---|---|---|
| completed dispatch reports `is_error` on an opaque contract | `Unknown`, session fatal | ambiguity recorded, then resolved |
| `state_payload_digest` fails after the lease is `Running` | returns with the lease stuck `Running` | records `ResultPersistenceFailed` first |
| pre-start cancellation, post-hook `not_applicable()` errors | returns before the tool lease is closed | tool lease is closed first |

The last two were surfaced by the cross-audit panel (Codex and Kimi both named
the digest path independently). They are defensive: I could not construct an
input that makes `state_payload_digest` fail, so **neither has a red test** —
stated plainly rather than papered over.

### The fix

**Read / Grep / Glob no longer claim to be opaque.** `Opaque` is documented as
"the host cannot prove whether an *interrupted* invocation took effect". These
three mutate nothing, so no interrupted invocation can leave anything behind.
They now declare `RepeatSafe`, and their failures — refusals included — are
plain terminal failures. This removes the UAT's exact reproducer at the root,
with no reclassification anywhere.

**Genuinely opaque tools keep their ambiguity, and it gets resolved.** For
Bash, MCP, plugins and network tools the ambiguity is real: the tool may have
changed the world before failing. The `Unknown { AmbiguousFailure }` record is
still written, and is then resolved in the same process by
`ToolExecutionResolved { Failed, Reconciler { "in-process:dispatch-completion" } }`.
The durable record ends up carrying **more** than either alternative: the
ambiguity, its evidence, the resolution, and the source that made it.

This fires **only when the dispatch ran to completion**. Timeout, panic,
observed cancellation, and a tool's own `ToolEffectDisposition::Unknown` all
stay nonterminal and reconcile-only, because nothing watched those finish. A
crash between the two appends still leaves `Unknown` for recovery, exactly as
before. The existing controls prove the fix did not over-reach:
`dispatcher_timeout_is_durable_unknown`, `dispatcher_panic_is_durable_unknown`
and `cooperative_cancellation_after_start_is_durable_unknown` are all still
green, unmodified.

### Tests I changed, and why — read this critically

Four existing tests asserted the defective invariant. I did not weaken or
delete any; each got a **strictly stronger** replacement. Flagged explicitly
because "the agent edited the test" is the shape that should always be
challenged:

* `opaque_reported_error_is_unknown_not_false_terminal_failure` asserted the
  record ends `Unknown`. Now asserts the ambiguity is still on the record (via
  `resolution_source`, which the reducer accepts **only** after
  `ToolExecutionUnknown` — so it cannot be faked) **and** the state is terminal
  **and** `TurnCommitted` succeeds.
* The three `actual_{mcp,plugin,script}_adapter_error_is_durable_unknown` tests
  shared one helper with the same assertion; same replacement.
* `newly_unknown_tool_stops_the_live_turn_before_another_provider_call`
  asserted that a tool which *ran and reported an error* must abort with
  `AgentError::SessionAuthority` and never reach the provider again. That is
  D1 written as an intended invariant. **Split in two:** a completed error now
  proves the opposite (turn survives, model gets a round, nothing needs
  reconciliation), and the original rule is kept verbatim for an effect nothing
  observed finishing — a tool that panics mid-dispatch.

---

## D3 — `backend = "plaintext"` disables every turn behind a misdirecting error

`open_confidential_store` refusing `Plaintext` is **unchanged**. No bypass, no
flag, no downgrade. What changed is when the refusal is reported and what it
says.

Two separate defects were behind the one string:

1. `recovery_confidential.rs` did `map_err(|_| Unavailable)` on every failure,
   discarding the real cause — including the store's own message, which already
   said "plaintext credentials are not permitted for confidential material".
2. `preflight` runs per turn, so a statically-decidable config conflict was
   discovered on the user's first prompt.

**Fix.** The rule is now one predicate,
`CredentialsBackend::supports_confidential_material()`, and
`open_confidential_store` uses it so there is a single source. A journaled
session refuses to open on a plaintext backend (`AgentEngine::init_session`,
before the session directory is created). The error enum carries one cause each
— `PlaintextBackendRejected`, `NoSecureBackendAvailable`,
`SecureStoreUnreadable`, `MissingRecoveryKey` — with the setting to change
named in each.

Naming the configured backend in an error is not a disclosure: the value is in
the operator's own cleartext config. Key material, ciphertext, AAD and paths
are still never rendered, and there is a test asserting so across all variants.

`isolated_profile_without_secure_store_fails_before_turn_or_provider_intent`
(wcore-cli) passes unmodified — the split message still satisfies its
actionability assertions.

---

## Panel decision and dissent (brief §4)

**D1 — 2 of 3 for in-process resolution; I took the majority, amended by the
dissent.** Gemini 3.1 Pro and Kimi K3 both returned `PANEL_POSITION=1`. Codex
5.6 Sol returned `other` and argued the strongest point made by anyone:

> "Fix (1) launders uncertainty. Dispatch completion proves that a result
> returned; it does not prove an opaque effect failed. Recording Unknown then
> immediately resolving it from the same evidence adds no knowledge."

That is a fair charge against the resolution step, and it is why the shipped
fix is not just option 1. Codex's own counter-proposal — classify by what the
tool can attest, and stop letting read-only tools inherit `Opaque` — is
adopted as the *primary* mechanism. The UAT's actual reproducer is now fixed by
correcting a false contract, not by resolving anything. Resolution survives only
for tools where the ambiguity is genuine and unattestable, where the honest
choice is between recording ambiguity-then-resolution and recording a bare
`Failed` that erases it. Codex's third point — that `Reconciler` should not
launder a same-evidence decision — is answered by naming the reconciler
`in-process:dispatch-completion`, so an auditor can filter it. Gemini wanted a
new `ToolResolutionSource` variant instead; rejected because that enum is
serialized into the journal and the wire contract, and regenerating contract
fixtures is a release action this lane must not take (brief §0).

**D3 — unanimous `AB`.** All three panelists independently: fail early at
config validation **and** split the error causes; all three said naming the
configured backend leaks nothing and called the current collapse over-applied
threat modelling; all three said "wrong passphrase vs no backend" is the same
class as `gpg` reporting a bad passphrase.

**Internal adversarial pass, against the consensus:** the panel's shape (A)
was "reject at config parse". I rejected that half. There is no `Config::validate()`
today, wiring one would touch the fenced CLI entrypoints, and a parse-time hard
failure would break `--doctor` and `--init-config` — the exact commands a user
needs to *diagnose* the problem. Enforcement therefore sits at session open,
which is still before any prompt, is a pure config predicate with no side
effects, and leaves diagnostic commands working. Kimi's own dissent (a carried
config that is never used with journaling would now be rejected) is bounded by
the same choice: only a journaled session refuses.

---

## Gates — real numbers, all on hetzner Linux, all read

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy -p wcore-agent -p wcore-tools -p wcore-config --all-targets` | clean (only the pre-existing `imap-proto` future-incompat note) |
| `cargo test -p wcore-agent` (lib + integration, `--test-threads=1`) | **2341 passed, 0 failed**, 3 ignored |
| `cargo test -p wcore-tools` | 0 failed across 20 targets (989 in the lib) |
| `cargo test -p wcore-config` | 525 passed, 1 pre-existing flake (below) |
| `cargo test -p wcore-cli --test f14_sigkill_recovery` | 11 passed, 0 failed |

### Red before, green after

D1, `cargo test -p wcore-agent --lib orchestration::d1_refusal_terminal_tests`
against unmodified HEAD — 4 failed, 1 passed:

```
refused_read_leaves_turn_committable ... FAILED
  got Unknown { reason: AmbiguousFailure { error: "Refused to read
      C::\\work\\needle.txt: path must be absolute" }, … }
failed_grep_leaves_turn_committable ... FAILED
failed_glob_leaves_turn_committable ... FAILED
opaque_shell_error_leaves_turn_committable ... FAILED
approval_denial_control_leaves_turn_committable ... ok      ← the UAT's control
```

The denial control passing at base is the UAT's own decisive control,
reproduced: denial was already terminal, refusal was not.

D3, `cargo test -p wcore-agent --test d3_credentials_backend_test` against
unmodified HEAD — 1 failed, 2 passed:

```
plaintext_backend_is_rejected_when_the_persisted_session_opens ... FAILED
  a plaintext credentials backend cannot open a journaled session: ()
auto_backend_still_opens_a_persisted_session ... ok
keyring_backend_is_not_rejected_statically ... ok
```

After the fix: 353/353 in `orchestration::`, 3/3 in the D3 target.

---

## Live evidence (brief §3.1) — real binary, Linux, A/B against the base build

Both binaries built from source on hetzner. Provider is a deterministic
OpenAI-wire stub that emits the UAT's exact malformed `Read` call, then plain
text, so a session that survives says so out loud. **The API key used is the
obviously-fake `sk-FAKE-uat-key-not-real-0000`; no real credential was read,
printed or transmitted.**

**D1 — base binary (`ac94b1d5`), the defect:**

```
> Read({"file_path":"C::\\wl-uat-work\\needle.txt"})
  X Refused to read C::\wl-uat-work\needle.txt: path must be absolute
error: Session persistence authority unavailable: invalid journal state transition:
       turn turn-4e69031a-… has nonterminal tool execution tool-execution-fcb8c372-…
EXIT=1     provider requests: 1
```

Identical to the Windows transcript, on Linux. **D1 is not Windows-specific.**

**D1 — fixed binary, same script, same stub:**

```
> Read({"file_path":"C::\\wl-uat-work\\needle.txt"})
  X Refused to read C::\wl-uat-work\needle.txt: path must be absolute
* TURN_SURVIVED_THE_REFUSAL
EXIT=0     provider requests: 2
```

The refusal still refuses. The session lives, and the model gets the round it
needs to correct itself.

**D3 — the UAT's A/B, same profile, same fake key, one variable:**

```
BASE   backend=auto      exit=1  Provider error: API error 401 … invalid x-api-key
BASE   backend=plaintext exit=1  Session persistence authority unavailable: secure
                                 recovery storage is unavailable; configure an OS
                                 keyring or encrypted credentials vault
FIXED  backend=auto      exit=1  Provider error: API error 401 … invalid x-api-key
FIXED  backend=plaintext exit=1  Error: credentials.backend is set to "plaintext",
                                 which cannot hold the confidential key that durable
                                 session recovery requires. Set credentials.backend to
                                 "keyring" or "encrypted-file", or disable session
                                 persistence
```

The `auto` arm is byte-identical across both builds — the security refusal and
the provider path are untouched.

A third live run produced the *other* cause distinctly, unprompted: an isolated
profile with no keyring and no vault passphrase now says "no OS keyring was
usable and no encrypted credentials vault is unlocked", which under the base
build was the same string as the plaintext case. That is D8's collapse
partially closed as a side effect, on live evidence.

---

## Open, and what I did NOT do

* **Pre-existing, not mine — parallel test isolation in `wcore-agent`.** Run in
  parallel the lib suite fails 13–22 tests with a *different set each run*, at
  base and on this branch alike (base 22 then 13; branch 19 then 14; several
  panic in the process-global crash-injection cell at `orchestration/mod.rs:146`).
  Single-threaded, base is 2098/0 and this branch is 2107/0 — clean
  apples-to-apples. MEDIUM, backlog, non-blocking per brief §5.
* **Pre-existing, not mine — `wcore-config` max-tokens flake.** One test in that
  family fails per full-suite run and passes in isolation; at base it was
  `test_resolve_cli_max_tokens_marks_explicit`, on this branch
  `test_resolve_omitted_max_tokens_reads_as_not_explicit`. Shared-environment
  race. MEDIUM, backlog.
* **Two lease fixes have no red test** (`state_payload_digest` failure,
  post-hook `not_applicable()` failure). Defensive only; I could not construct
  inputs that reach them and did not manufacture a test that pretends otherwise.
* **D2 untouched.** No resume/reconcile surface modified.
* Neither `crates/wcore-cli/src/lib.rs` nor `main.rs` was edited — the §6 fence
  did not need to be entered. `init_session` already returns `Result` and its
  `main.rs` call site propagates with `?`.
* `wcore-contract generate` was not run. No protocol or wire-contract change:
  the D1 fix reuses the existing `ToolResolutionSource::Reconciler` variant
  precisely to avoid one.
* No test weakened, ignored, allowed, re-gated, deleted, or given a longer
  timeout.
