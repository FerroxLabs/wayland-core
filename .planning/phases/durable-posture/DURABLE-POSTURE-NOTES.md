# durable-posture — running NOTES (append after every measurement)

Lane: `lane/durable-posture`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-durable-posture`.
Base integration: `c9ab048b952c5bc74c75ea8f76df06788408de59` (asserted via
`/usr/bin/git rev-parse HEAD`, unproxied).

## T+0 — premise verification (Mac, unproxied tools only)

| # | Brief premise | Verdict | Evidence |
|---|---|---|---|
| 1 | `c73ac417` = "a host with no keyring now runs, degraded and announced" | **HOLDS** | `git log -1 c73ac417df54ec9069a2c376d72feba218f8e85c`, dated Thu Jul 30 20:20:56 2026 |
| 2 | `906287e1` (2026-07-16) = "feat(recovery): seal interrupted turn state" | **HOLDS** | `git log -1 906287e1790ab2e0c8a6f1f71940e9acc2b55c75`, Thu Jul 16 12:54:28 2026 |
| 3 | `crates/wcore-cli/tests/f14_sigkill_recovery.rs:1106` asserts no provider + no turn without secure store | **HOLDS, with an addition the brief omits** | fn `isolated_profile_without_secure_store_fails_before_turn_or_provider_intent` starts at line 1106. It asserts `engine_error`, `retryable=false`, `fixture.observation().requests.is_empty()` — AND at line 1143 it does `fs::read(&journal).expect("read preflight-failed session journal")`, i.e. **it requires the journal FILE to exist** and merely be free of `turn_started` / the prompt. See §"Premise 3 addendum". |
| 4 | `WAYLAND_VAULT_PASSPHRASE` / `_FD` live in `crates/wcore-config/src/credentials.rs` | **HOLDS** | credentials.rs:883 (`_FD`), :893 (env var), :1104 `vault_unlock_material_present()` |
| 5 | Test passes at `b8311575`, fails at integration head at `f14_sigkill_recovery.rs:204` | **UNVERIFIED at T+0** — needs hetzner (test is `#[cfg(target_os = "linux")]`, cannot run on the Mac) | — |

Instrument control for the greps above: the same `/usr/bin/grep -rn WAYLAND_VAULT_PASSPHRASE crates/`
returned 40 hits across 14 files (known-positive alive); the same grep restricted to
`crates/wcore-config/src/` returned 31 hits with rc=0.

### Premise 3 addendum — the July-16 test is NOT purely "must not run"

The brief characterises `isolated_profile_without_secure_store_fails_before_turn_or_provider_intent`
as asserting the product "must NOT reach the provider and must NOT start a turn". True, but
incomplete, and the omission is load-bearing for this lane: the test **also requires the session
journal file to exist and be readable** (`fs::read(&journal).expect(...)`, line 1143). So the
July-16 posture was *not* "refuse and write nothing" — it was **refuse, but still journal the
refusal**. That is much closer to what this lane is being asked to build than the brief implies,
and it means the c73ac417 degrade (which creates no `sessions/` directory at all) reverses TWO
properties, not one.

## Architecture as measured (integration head c9ab048b)

The decision chain, single point, `wcore-config/src/config.rs:2478`:

```
Config::resolve_inner
  -> durable_sessions_must_be_disabled(session.enabled, backend, || confidential_recovery_storage_available())
       == session_enabled && backend.supports_confidential_material() && !measure_availability()
  -> resolved.session.enabled = false ; record_durable_sessions_disabled_by_host()
```

`confidential_recovery_storage_available()` (config.rs:2544) →
`credentials::confidential_backend_available` (credentials.rs:1502) →
`confidential_backend_plan` + `select_confidential_backend` with availability inputs
`(keyring_available, vault_unlock_material_present())`.

`vault_unlock_material_present()` (credentials.rs:1104) is **only**:

```rust
#[cfg(unix)] if env WAYLAND_VAULT_PASSPHRASE_FD is set { true }
env WAYLAND_VAULT_PASSPHRASE is set
```

**So the whole cliff is one predicate.** `CredentialsBackend::Auto` offers exactly two
confidential candidates — `Keyring{service}` and `EncryptedFile{cipher,kdf}` — and the
EncryptedFile candidate is judged unavailable purely because nobody typed a passphrase.
There is no third candidate and no auto-provisioned key. That is the false dichotomy the
lane exists to dissolve, and it is localised to one function.

Also relevant: a backend marker is pinned at
`.credentials.confidential-backend.json` on first successful open
(`resolve_confidential_backend_with_availability`, credentials.rs:602-611), and a pinned
backend that is later unavailable is a **hard error**, not a downgrade
(credentials.rs:506-510). That is the existing precedent for task 2 (refuse when a journal
exists but its key does not) — the mechanism already exists at the credentials layer; the
question is whether it covers the *journal* rather than the *store*.

## T+1 — premise 5 VERIFIED on hetzner, with an addition

`cargo test -p wcore-cli --test f14_sigkill_recovery -- --exact
isolated_profile_without_secure_store_fails_before_turn_or_provider_intent` at
`c9ab048b`:

```
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 11 filtered out
panicked at crates/wcore-cli/tests/f14_sigkill_recovery.rs:204:9
  left: None
 right: Some("f1400000000000000000000000000000")
```

Exactly the line the brief named. **But the failure value is the finding**: `left: None` means the
`ready` frame carries **no `session_id` field at all**. `ProtocolEvent::Ready.session_id` is
`Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]`
(`wcore-protocol/src/events.rs:559-562`), so the degraded `ready` is byte-identical in shape to a
legacy producer's. A `--json-stream` host is told nothing whatsoever. Full capture:
`evidence/01-head-f14-test-FAILED.log`.

## T+2 — what the key actually protects (this reframes the whole lane)

The journal is **not encrypted**. It is a framed JSONL event log, `0600` inside a `0700`
directory, and the reader *refuses to open it* if `mode & 0o077 != 0` or the owner uid differs
(`session_journal.rs:712/744/819`).

The confidential store holds exactly **one 32-byte key**, `wayland-core.recovery.prepared-request.v1`
(`recovery_confidential.rs:15`), used to seal exactly **one field**:
`RecoveryCheckpoint.sealed_prepared_request` — the exact prepared provider request, which is what
makes AUTOMATIC REPLAY possible.

Meanwhile the audit trail is already keyless and already exists. `LEGACY_EVENT_TYPES`
(`session_journal.rs:2376-2426`) contains write-ahead pairs for **every effect boundary the brief
names**: `provider_attempt_prepared/started/finished/not_started`,
`tool_intent_recorded` / `tool_execution_started/finished/unknown/resolved`,
`approval_requested/resolved`, `delivery_prepared/started/finished/not_started`,
`turn_started/committed/failed`. `journal_provider.rs` writes them at the real send boundary and
`with_dispatch_id` is OPTIONAL — the v1 `ProviderAttemptPrepared` variant with no dispatch id is
already a legal event.

**So the false dichotomy is even weaker than the brief argued.** It is not "no key ⇒ no journal";
it is not even "no key ⇒ no audit trail". The only thing a missing key costs is *replay*. The
coupling that turns that into amnesia is one validation rule: a `ProviderDispatch` checkpoint is
rejected unless `sealed_prepared_request` is present (`recovery.rs:331`).

## T+3 — cross-audit panel: 3/3 for a FOURTH option, unanimous

I put four options to the panel, including one the brief did not consider (**D: journal without
the seal**). All three external legs returned `PANEL_CHOICE=D`, each with a substantive
multi-page answer (alive; no zero-byte votes). Raw captures: `evidence/03..05`.

Convergent points across all three, none of which I prompted for:

- **C (today's amnesia) is the only option that grants UNRECORDED EXECUTION.** codex: "disable
  credentials backend becomes obtain unrecorded execution."
- **B (fail-closed by default) recreates the release blocker** and is not a real default.
- **A (machine-local key file) buys one thin property** — survival of *partial* disclosure, where
  the journal is copied and the key is not — at the cost of a key-management lifecycle. kimi's
  killing argument: "A spends a key-management lifecycle to protect a field that D simply
  declines to persist."
- **Q4 CORRECTS THE BRIEF.** The brief says refuse when a journal exists but its key does not.
  All three legs say refuse the **SESSION**, not the **PROCESS**. codex: "no global process
  brick, but no automatic continuation from unknowable state." kimi: a global refusal hands an
  attacker who can suppress the keyring an availability kill, converting a confidentiality attack
  into a DoS. See §Deviations in the SUMMARY.
- **Q5 costs D some of its claim.** codex and kimi both hold that *no* local record can prove a
  remote effect occurred: written before the call it proves intent only; written after, a crash
  can land between the effect and the record. D **bounds** the ambiguity and routes it to a
  decider; it does not resolve it. Resolving it needs provider idempotency, which is orthogonal
  to A vs D.

### Internal adversarial pass (arguing AGAINST the consensus)

1. **The unanimity is partly an artifact of my framing.** I authored option D, listed it last, and
   described it more favourably than A/B/C. Three legs agreeing with the option its author framed
   best is weaker evidence than three independent convergences. Discount accordingly.
2. **Neither codex nor kimi actually voted for pure D.** codex: "I would make D the default and
   optionally offer A as an explicit enhancement." kimi: "D, plus fix fact 8, plus treat Q4 per
   above." The real consensus is *D as the mandatory baseline, A as an opt-in*, plus riders.
3. **codex adds a rider that is arguably bigger than the choice itself**: "if the journal cannot
   be appended and durably flushed, external execution must fail closed." That is a write-ahead
   ordering invariant nothing in this lane implements or tests.
4. **D is a recovery-schema change.** `recovery.rs:331` must learn a dispatch state with no seal,
   and the resume path must refuse rather than continue. That is engine-loop work, not a config
   change, and it is the one item here I could not land safely in a single lane.
