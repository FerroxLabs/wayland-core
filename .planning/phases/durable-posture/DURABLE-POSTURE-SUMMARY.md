# durable-posture — LANE SUMMARY

Branch `lane/durable-posture`. Merge base `c9ab048b952c5bc74c75ea8f76df06788408de59`.
Build/test host: `hetzner-dsm`, worktree `/root/wayland-durable-posture`, debug profile only.
Every figure below was read from a file with the Read tool, never from a piped `rtk` render.

## Honest verdict

**Partially achieved, and the unachieved part is the headline task.**

- Task 3 (degrade must be permitted by policy) — **DONE, live-proven, both directions.**
- Task 4 (two tests, not one) — **DONE, both directions.**
- Task 5 (crash-boundary, exhaustive zero-residue) — **DONE for 5 boundaries; outbound
  delivery UNRUN and named.**
- Task 6 (per-turn signal) — **DONE for the CLI/TUI and for `--json-stream` via an
  already-contracted frame. The typed `ready` field is a seam request, not built.**
- Task 1 (dissolve the dichotomy — journal without a secure store) — **NOT BUILT.** Designed,
  cross-audited 4 ways, and the design is recorded. It is an engine-loop and recovery-schema
  change, and landing it half-done would have been worse than not landing it.
- Task 2 (refuse when a journal exists but its key does not) — **NOT BUILT, but MEASURED LIVE
  and it is worse than the brief assumed.** See HIGH-1.

## 1. Premises in my brief: which held

| Premise | Verdict |
|---|---|
| `c73ac417` = the keyring-less degrade fix | HOLDS |
| `906287e1` (2026-07-16) = "seal interrupted turn state" | HOLDS |
| `f14_sigkill_recovery.rs:1106` asserts no provider, no turn | **HOLDS BUT INCOMPLETE — see below** |
| `WAYLAND_VAULT_PASSPHRASE`/`_FD` in `wcore-config/src/credentials.rs` | HOLDS (`:883`, `:893`, `:1104`) |
| Test fails at head, panicking at `f14_sigkill_recovery.rs:204` | HOLDS — reproduced at `c9ab048b` |

**Correction 1 — the July-16 test was not "refuse and write nothing".** It also did
`fs::read(&journal).expect("read preflight-failed session journal")` at line 1143: the journal
FILE had to exist, and merely be free of `turn_started` and of the prompt. So the July-16 posture
was *refuse but still journal the refusal*, and `c73ac417` reversed **two** properties, not one.
That is much closer to what this lane was asked to build than the brief implies.

**Correction 2 — agrees with the coordinator's own correction, found independently here before
it arrived.** The `:204` panic is the READY HANDSHAKE, not the journal. `left: None` — the
degraded `ready` frame carries **no `session_id` at all**, because
`ProtocolEvent::Ready.session_id` is `Option<String>` with
`skip_serializing_if = "Option::is_none"` (`wcore-protocol/src/events.rs:559-562`). Confirmed a
second time against the **real binary**, outside any test harness:
`FRAME_TYPES[degrade-default]: ['ready(session_id=None)']` (`evidence/16`).

**Correction 3 — a claim `c73ac417` made about itself is false in the common case.** Its message
says a degraded run "creates no `sessions/` directory at all". On any profile where the directory
already exists — which includes every `tempenv::build` profile, and any host that has ever run
durably — it exists and stays EMPTY. An `exists()` assertion reds for reasons unrelated to the
product; mine did, on its second run. The durable property is emptiness, and that is what is now
asserted, with a control proving the count is not always zero.

## 2. The measurement that reframes the whole lane

The journal is **not encrypted**. It is a framed JSONL event log at `0600` inside a `0700`
directory, and the reader refuses to open it if `mode & 0o077 != 0` or the owner uid differs
(`session_journal.rs:712/744/819`).

The confidential store holds exactly **one 32-byte key**
(`wayland-core.recovery.prepared-request.v1`, `recovery_confidential.rs:15`) protecting exactly
**one field**: `RecoveryCheckpoint.sealed_prepared_request`, which is what makes AUTOMATIC REPLAY
possible.

The audit trail is **already keyless and already exists**. `LEGACY_EVENT_TYPES`
(`session_journal.rs:2376-2426`) carries write-ahead pairs for every effect boundary the brief
names — `provider_attempt_prepared/started/finished/not_started`, `tool_intent_recorded`,
`tool_execution_started/finished/unknown/resolved`, `approval_requested/resolved`,
`delivery_prepared/started/finished/not_started`, `turn_started/committed/failed`. They are
written by `journal_provider.rs` at the real send boundary, and `with_dispatch_id` is optional —
the v1 `ProviderAttemptPrepared` with no dispatch id is already a legal event.

**So the dichotomy is even weaker than the brief argued.** It is not "no key ⇒ no journal", and it
is not even "no key ⇒ no audit trail". A missing key costs only *replay*. What turns that into
amnesia is one validation rule: a `ProviderDispatch` checkpoint is rejected unless
`sealed_prepared_request` is present (`recovery.rs:331`). Everything else follows from
`Config::resolve` setting `session.enabled = false`, which makes `session_journal` `None` and
sends the turn down the no-journal branch at `engine.rs:6263`.

## 3. Cross-audit panel — 3/3, unanimous, for a FOURTH option

I put four options to the panel, including one the brief did not consider. All three external
legs answered at length (no zero-byte votes; raw captures `evidence/03..05`; question
`evidence/02`).

- **A** auto-provision a machine-local 0600 key file, silently.
- **B** the same, but fail-closed unless the operator opts in.
- **C** status quo (amnesia).
- **D** journal WITHOUT the seal: keep the keyless effect record, omit the one field that needs a
  key, and report the turn as explicitly unreplayable.

| Leg | Verdict |
|---|---|
| codex (gpt-5.6-sol) | `PANEL_CHOICE=D` |
| gemini (3.1-pro-preview) | `PANEL_CHOICE=D` |
| kimi (K3) | `PANEL_CHOICE=D` |

Verbatim, the load-bearing lines:

> **gemini:** "Option C (the amnesia degrade) is outright negligence… That is a critical
> vulnerability masquerading as a graceful degradation." / "B is cowardice. It fails closed by
> default, which recreates the release blocker… while pushing the engineering failure onto the
> operator's configuration file." / "Under A, the attacker forces the system to write the
> sensitive payload using a local key they can immediately steal. Under D, the downgrade results
> in the sensitive payload being dropped from disk entirely."

> **codex:** "The correct invariant is: **no external effect may occur unless a durable intent
> record exists first**. Failure to encrypt a replay payload may reduce recovery capability; it
> must not disable the audit trail." / "C is indefensible. It turns loss of a confidentiality
> facility into loss of the audit trail." / "The JSON `ready` frame must disclose something like
> `journal=metadata_only`, `sealed_recovery=false`, and `replay=operator_required`. A one-time
> stderr notice is not an API contract, especially for a three-week-old daemon." / "no global
> process brick, but no automatic continuation from unknowable state."

> **kimi:** "Fact 7 … is not a law of nature; it's a self-imposed coupling between two different
> things: the journal as an *audit trail* … and the journal as a *recovery payload*." / "A spends
> a key-management lifecycle to protect a field that D simply declines to persist." / "A security
> default that the standard deployment must disable to function is not a default; it's a support
> ticket generator." / "an attacker who can make the keyring unavailable can now halt the product
> — converting a confidentiality attack into an availability attack."

**On Q1 (is a 0600 key beside a 0600 journal theatre?) all three converge:** not pure theatre, but
narrow. It defends only *partial* disclosure — a support engineer attaching the journal to a
ticket, a log shipper globbing `*.journal`, `scp host:.../sessions/x.journal`. Against the same-uid
or root attacker implied by the mode checks it is worth exactly zero, and any wholesale backup
takes the key too.

### My internal adversarial pass, arguing AGAINST the consensus

1. **The unanimity is partly an artifact of my framing.** I authored D, listed it last, and
   described it most favourably. Three legs agreeing with the option its author framed best is
   weaker than three independent convergences. Discount it.
2. **Neither codex nor kimi voted for pure D.** codex: "I would make D the default and optionally
   offer A as an explicit enhancement." kimi: "D, plus fix fact 8, plus treat Q4 per above." The
   real consensus is *D as the mandatory baseline, A as an opt-in, plus riders*.
3. **codex's rider is arguably bigger than the choice.** "If the journal cannot be appended and
   durably flushed, external execution must fail closed" is a write-ahead ordering invariant that
   nothing in this lane implements or tests.
4. **Q5 costs D part of its claim, and both legs said so.** No local record can prove a remote
   effect: written before the call it proves intent only; written after, a crash can land between
   the effect and the record. D **bounds** the ambiguity and routes it to a decider; it does not
   resolve it. Resolving it needs provider idempotency, orthogonal to A vs D.

**Where the panel CORRECTS my brief:** the brief says refuse when a journal exists but its key
does not. All three legs say refuse the **SESSION**, not the **PROCESS** — a global refusal hands
an attacker who can suppress the keyring an availability kill, and bricks a server whose D-Bus
merely restarted. I have adopted the panel's narrower form in the design and say so rather than
implementing the brief as literally written.

## 4. What I built

### 4.1 `[session] require_durability` — the degrade becomes a capability the operator can decline

`wcore-config/src/config.rs`. `host_durability_disposition()` is a pure function returning
`Keep | Degrade | Refuse`; the host decides whether durability is DELIVERABLE, the operator decides
what happens when it is not. `Refuse` bails at config resolution with
`DURABILITY_REQUIRED_REFUSAL`, a single `const` naming the cause and all three ways out including
the way back to the degrade.

It also closes a downgrade path nobody had noticed. The session merge has a branch that replaces
the **whole** global session block when a project file sets a custom `directory`. A project
`.wayland-core.toml` is untrusted — it travels with a cloned repo — so a repo that changed nothing
but the session directory would have silently cleared the operator's global policy.
`require_durability` is now tighten-only in both branches, matching the `allow_no_sandbox` clamp in
the same function, with a known-negative row proving the merge is not hardcoded `true`.

### 4.2 Per-turn degrade signal

`wcore-agent/src/engine.rs`, `announce_host_forced_degrade_for_this_turn()`, called on the
no-journal branch. Emits through `OutputSink::emit_info` — the terminal for a TUI/CLI run, an
already-contracted `ProtocolEvent::Info` for a protocol host, so **no contract change and no
fixture regeneration**. Conditioned on `durable_sessions_disabled_by_host()`, not on the journal
being absent: an operator who wrote `[session] enabled = false` asked for this and must not be
nagged once per message.

### 4.3 The two tests, and the crash matrix

`wcore-cli/tests/f14_sigkill_recovery.rs` (fenced to this lane; the failing July-16 test is
replaced, not deleted):

- `without_secure_store_an_operator_who_requires_durability_gets_a_refusal` — **degrade
  FORBIDDEN.** Carries forward the assertion that mattered (`fixture.observation().requests` is
  empty) and adds that the refusal reaches a `--json-stream` host as exactly one non-retryable
  error frame on stdout naming the cause. States plainly the one property it does not carry
  forward: there is no journal to inspect, because the refusal precedes every engine — which is
  asserted, not dropped.
- `without_secure_store_the_default_runs_degraded_and_leaves_nothing_durable` — **degrade
  ALLOWED.** Two turns (one turn cannot distinguish a per-turn notice from a startup one), the
  provider reached exactly once per turn, the notice correlated to each turn's `msg_id`, and zero
  residue.
- `a_degraded_run_killed_at_any_effect_boundary_leaves_nothing_durable` — the crash matrix.

**Zero-residue is not a glob for `sessions/*.journal`.** The journal family is
`<id>.journal`, `<id>.wal`, `<id>.journal.snapshot`, `<id>.journal.authority`,
`<id>.journal.writer.lock`, the `.<id>.journal.effects/` directory and its
`.<digest>.<pid>.<seq>.tmp` temporaries. The walker reads every file under the profile home, both
by path family and by sweeping every file's bytes for the prompt, and counts `sessions/` entries
separately.

## 5. Crash-boundary results

SIGKILL into a degraded run at five boundaries, profile read AFTER the kill with no clean
shutdown, so temps, WALs and partially-renamed artifacts are still on disk.

| Boundary | Artifacts | Prompt leaks | `sessions/` entries | Provider requests |
|---|---|---|---|---|
| `before-any-turn` | 0 | 0 | 0 | 0 (expected) |
| `provider-request-sent-no-headers` | 0 | 0 | 0 | 1 |
| `provider-stream-partially-consumed` | 0 | 0 | 0 | 1 |
| `awaiting-tool-approval` | 0 | 0 | 0 | 1 |
| `tool-executing` (child proven running via its own marker file) | 0 | 0 | 0 | 1 |

Anti-vacuity, because five zeroes are five absences:

- Each boundary asserts the provider-request COUNT it should have produced, and the loop asserts
  the total is **4, not 0** — an actor that never launched is a dead instrument.
- A **DURABLE** run is killed at the same partial-stream boundary and is REQUIRED to leave
  residue. Without it, a walker that always returns empty satisfies every assertion in the test.

## 6. Both-directions proofs — every new gate reddened, then restored

Mutate the production code, observe red, restore, observe green. The mutation script aborts if
its target string is not present exactly once, so a mutation that silently did nothing cannot be
reported as a gate holding. Every cell reports its executed count, so no run of zero tests is
being read as a pass.

**`wcore-config` (`evidence/07`)** — a clean 3×3 diagonal:

| Mutation | policy table | merge clamp | refusal text |
|---|---|---|---|
| M1 drop `require_durability` from the disposition | **RED 101** | green | green |
| M2 restore the wholesale `project.session` branch | green | **RED 101** | green |
| M3 delete the way-back-to-degrade remedy | green | green | **RED 101** |
| (restored) | green | green | green |

**`f14` (`evidence/13`)**:

| Mutation | FORBIDDEN | ALLOWED | CRASH MATRIX |
|---|---|---|---|
| F1 remove the per-turn announcement | green | **RED 101** | green |
| F2 stop degrading (keep `session.enabled`) | green | **RED 101** | **RED 101** |
| F3 ignore `require_durability` | **RED 101** | green | green |
| (restored) | green | green | green |

F2 reddening both residue tests and neither policy test is the expected shape: both depend on the
degrade actually producing zero residue, and neither policy test does.

## 7. Gates

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **0** |
| `cargo metadata --locked` | **0** |
| `cargo check --workspace --all-targets` | **0** |
| `cargo clippy` over the 7 touched targets, `-D warnings` | **0 on all 7** |
| `cargo test -p wcore-config --lib` | **583 passed; 0 failed; 0 ignored; 0 filtered out** |
| `cargo test -p wcore-cli --test f14_sigkill_recovery` | **13 passed; 0 failed; 1 ignored; 0 filtered out** |
| `cargo test -p wcore-agent --lib -- --test-threads=1` | **2252 passed; 0 failed; 3 ignored; 0 filtered out** |

The single `ignored` in f14 is the pre-existing `f14_seed_recoverable_turn_helper`, which is
`#[ignore]` by design and is invoked by re-exec — this lane invoked it directly and it reported
`1 passed` (`evidence/16`).

### `clippy --workspace -D warnings` is red, and it is NOT mine — fourth confirmation

Exits 101 on `wcore-agent/tests/cache_ledger_engine_test.rs:82` (`needless_update` on a
`TokenUsage`) and `wcore-agent/tests/user_model_identity_wire.rs:229/337/…`
(`needless_borrows_for_generic_args` on a `LocalBackend`). Both files are **byte-identical at the
merge base** — sha256 `5635034052ce3ca8` and `03ba694d77e2b7ab` at both ends — and neither struct
has anything to do with `SessionConfig`. Control that the comparison discriminates:
`crates/wcore-config/src/config.rs`, which I DID change, hashes `a82f375b…` at base and
`a5ec900d…` at head (`evidence/14`).

Because a scoped-clean clippy is itself an absence claim, the scoped run carries a
**known-positive control**: it deliberately also runs the two known-failing targets and requires
them to report `101` (`evidence/12`). They do. So the seven zeroes are the command working, not
the command being unable to fail.

### `wcore-cli` full suite: 2 reds, proven pre-existing by ABLATION

`cargo test -p wcore-cli` (all binaries) shows two failing targets:

- `failing_fixture::always_fails` — a fixture crate whose entire purpose is to fail. Not a red.
- `harness_regression`: `13 passed; 2 failed` — `r011_channels_auto_register_logs` and
  `r012_honcho_fallback_on_no_key`, both failing on absent log lines
  (*"Either RUST_LOG did not propagate or F-093 regressed"*), both reporting
  `failures: [CostMissing]`. Nothing to do with sessions, durability or credentials.

Rather than assert that from the topic, I ablated it (`evidence/19`): restored **all eight** of
this lane's changed files to the merge base in the hetzner worktree, rebuilt, re-ran the same
binary, then restored.

```
RC[with-lane]=101      test result: FAILED. 13 passed; 2 failed; 0 ignored; 0 filtered out
ABLATED porcelain=[M …engine.rs M …helpers.rs M …common/mod.rs M …anthropic.rs
                   M …compaction.rs M …openai.rs M …f14_sigkill_recovery.rs M …config.rs]
RC[without-lane]=101   test result: FAILED. 13 passed; 2 failed; 0 ignored; 0 filtered out
RESTORED porcelain=[]  RESTORED_SHA_MATCHES=yes
```

Identical counts and the identical two test names in both directions, with the ablation proven to
have taken effect (eight `M` entries) and the restore proven clean. **Pre-existing.**

### The `wcore-agent --lib` parallel reds are contention — confirmed again

The full-workspace-style parallel run gave `2231 passed; 21 failed`; the identical command
single-threaded at the identical commit gave `2252 passed; 0 failed`. That matches the figure the
lane brief records for `c73ac417` exactly.

## 8. Live evidence (real binary, `hetzner-dsm`, `evidence/16`)

`wayland-core 0.12.25`, snapshot copy so a concurrent cargo run could not swap it mid-experiment.
No provider credential; no arm sends a turn.

```
ARM degrade-default     rc=0  ready(session_id=None)          notice=1  sessions=0
ARM require-refuses     rc=1  error(init_failed, retryable=False)  notice=0  sessions=0
ARM vault-control       rc=0  ready(session_id='da...003')    notice=0  sessions=4
ARM require-with-vault  rc=0  ready(session_id='da...004')    notice=0  sessions=4
```

The refusal frame the host receives, verbatim from stdout:

> `Engine failed to start during init: [session] require_durability = true, but this host cannot
> protect a durable session: it has no usable OS keyring and no unlocked credentials vault.
> Refusing to start rather than running with no recovery journal. …`

Row 4 is the one that matters for the policy: **requiring durability on a host that CAN deliver it
costs the deployment nothing.** Rows 3 and 4 are also the controls that stop rows 1 and 2 passing
on a binary that degraded or refused unconditionally — the notice fires in exactly 1 of 4 arms.

### Instrument defect found and REPAIRED in-lane

v1 of the live script gave three arms session ids beginning `dp`, which the product rejects as
non-hex, so those arms died before the code under test and **both controls never ran**. Per
§6b-ii I repaired the instrument rather than writing it up: v2 uses hex ids and every arm now
reports `REACHED_CODE_UNDER_TEST`, so that exact defect cannot recur silently. v1 is retained as
`evidence/15`.

## 9. HIGH-1 — measured, NOT fixed: a resumed session with a lost key claims continuity it does not have

Seeded a profile with a **real recoverable turn** through the product's own production-shaped
seeder under a vault (`SEED_RC=0`, `1 passed`, 6 artifacts on disk). Relaunched the same profile
with `--resume <id>` and the vault removed:

```
ARM_RC[journal-exists-key-gone]=0
FRAME_TYPES: ["ready(session_id='da00000000000000000000000000005')"]
NOTICE=1
SESSION_ENTRIES=6   (unchanged, unread)
```

It starts, **emits `ready` carrying that session id**, degrades to no journal, and proceeds. The
six prior artifacts sit on disk untouched and unread. This is worse than the anonymous degrade:
the anonymous one at least says nothing, while this actively asserts continuity to the host for a
session the engine cannot read a single byte of. Every panel leg named this condition
independently. **Not fixed here** — the fix is the session-scoped refusal in §10.

## 10. Design of record for task 1 + task 2 (NOT BUILT)

For whoever picks this up. Adopted position: **D as the mandatory baseline, A as an operator
opt-in, refusal scoped to the session.**

1. `Config::resolve` stops setting `session.enabled = false`. It sets a new
   "replay protection unavailable" fact instead. The journal stays alive.
2. `engine.rs:6294`'s `recovery_request_protection.preflight()` stops being a hard gate when
   replay protection is unavailable.
3. `commit_provider_recovery_checkpoint` (`engine.rs:8509`) is skipped in that mode; the
   `dispatch_id` at `engine.rs:9963` is minted directly instead of taken from the checkpoint.
   `JournaledLlmProvider` continues to write the keyless
   `provider_attempt_prepared/started/finished` pairs — **no new event type is needed**, the v1
   no-dispatch-id variants already exist.
4. On restart, `recovery_plan()` sees a `turn_started` with no terminal and produces a non-`Ready`
   disposition, which `engine.rs:6280` already turns into the honest refusal *"session has an
   interrupted turn at journal cursor N; resume, reconcile, or cancel it"*. That refusal already
   exists and is the right one.
5. HIGH-1: a session whose journal holds a `ProviderDispatch` checkpoint that cannot be unsealed
   must be marked locked — refuse to RESUME **that session**, permit a new session under a new
   id, never reuse the old id or call the fork a resume. Do **not** brick the process.
6. codex's rider, unimplemented and untested here: if the journal cannot be appended and durably
   flushed, external execution must fail closed.

## 11. Seam request — protocol, for the orchestrator to serialize

Not actioned by this lane. `wcore-contract generate` was **not** run.

```
SEAM-REQUEST: name the degraded durability state on the wire
  Surface : ProtocolEvent::Ready (wcore-protocol/src/events.rs:559)
  Problem : under a host-forced degrade `session_id` is simply OMITTED
            (Option + skip_serializing_if). A host cannot tell that apart from a
            legacy producer or a malformed frame. Measured live at 0.12.25:
            FRAME_TYPES[degrade-default]: ['ready(session_id=None)'].
            The coordinator reports Desktop has landed a validator that reads
            this frame (lane/core-contract-integration @ 475bf309), which raises
            the cost of the ambiguity.
  Ask     : an explicit, typed durability posture on `ready`. codex's shape:
            journal = metadata_only | full | none
            sealed_recovery = bool
            replay = operator_required | automatic
  Note    : this lane deliberately did NOT force a contract change. The per-turn
            signal it shipped rides ProtocolEvent::Info, which is already pinned,
            so nothing needs regenerating to get the fix. The typed field is the
            better long-term answer, not a blocker on what landed.
  Also    : `f14_sigkill_recovery.rs` now ASSERTS today's shape
            (`ready.session_id == None` under degrade), so whoever changes it
            will red that line and read this.
```

## 12. What I did NOT do — every unrun cell

1. **Task 1 (D) not built.** Designed and cross-audited; §10.
2. **Task 2 not built.** Measured live as HIGH-1; §9.
3. **Outbound-delivery crash boundary UNRUN.** The f14 harness has no channel. 1 of 6 named
   boundaries.
4. **`--json-stream` per-turn notice not asserted end-to-end against a real Desktop host.** It is
   asserted against the real binary's stdout in the f14 test; no Desktop client was involved.
5. **macOS entirely unrun.** Both new f14 tests are `#[cfg(target_os = "linux")]`, following the
   test they replace — the D-Bus trick that makes the keyring deterministically absent is
   Linux-only, and a macOS Keychain is always present. I did **not** use the §0 Darwin exception;
   nothing here is Darwin-only behaviour.
6. **Windows unrun.** `f14_sigkill_recovery.rs` is `#![cfg(unix)]`.
7. **`wcore-agent` integration suites not run** beyond `--lib` and compilation
   (`check --workspace --all-targets` = 0). The engine change is one guarded, additive call.
8. **`clippy --workspace -D warnings` still 101** on two files proven byte-identical at base. Not
   repaired — out of scope, and repairing another lane's file is exactly the drive-by this program
   forbids.
9. **No contract regeneration, no PR, no merge, no tag, no issue closed, no credential supplied.**
   No `git rebase`, no `git reset --hard`, no `git clean`, no `git stash`. The only `git checkout`
   uses were named-path restores inside the hetzner worktree, during the mutation and ablation
   runs, each followed by a `porcelain=[]` assertion.
10. **Nothing merged into `plan/f20-unified-audit-repair`.** Every push was
    `git push gh HEAD:lane/durable-posture`.
