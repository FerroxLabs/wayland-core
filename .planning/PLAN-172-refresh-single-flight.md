# PLAN v2 — #172: cross-process single-flight for the OAuth refresh POST

Revised after a cross-audit panel. **v1's central failure policy was wrong and is
reversed.** Changes from v1 are marked **[AUDIT]**.

Base: `plan/f20-unified-audit-repair`. Target: `crates/wcore-agent/src/oauth/`.

## 1. The defect, from the code

`SingleFlightRefresh` (`flow.rs:619`) is `Mutex<Option<RefreshCell>>` owned per
`ChatGptTokenManager` (`chatgpt.rs:353`). It coalesces **within one process**.
`chatgpt.rs:477 refresh()` POSTs the token endpoint; ChatGPT refresh tokens
**rotate and are single-use**. Two processes on one profile near expiry both POST;
the second gets `invalid_grant`.

**[AUDIT] The consequence is worse than `invalid_grant`.** Under RFC 6819 §5.2.2.3
and the OAuth 2.1 BCP, replaying a single-use refresh token is treated as theft,
and the sanctioned response is to **revoke the entire authorization grant** —
including the tokens the winner just received. Both processes logged out, not one
turn failed. Every policy decision below follows from this.

## 2. Measured constants (verified, not assumed)

| | value | source |
|---|---|---|
| POST timeout | **20 s** | `chatgpt.rs:60`, `xai.rs:64` |
| Lock acquisition spin | **10 s** (200 × 50 ms) | `credentials.rs` MigrationLock |
| Staleness steal | **60 s** | same |
| Acquire primitive | **`std::thread::sleep`** | same |

**[AUDIT] The acquisition wait (10 s) is shorter than the hold time (up to 20 s).**
Any winner POST slower than ~10 s makes every concurrent loser exhaust the wait.
v1 would therefore have fired its fallback *on the happy path*, on a slow network.

## 3. What "fixed" means

- **P1** Two concurrent processes near expiry → **exactly one** POST, **both** end
  with a working token.
- **P2** Single process: unchanged behaviour, no added latency.
- **[AUDIT] P3** No path ever POSTs a refresh token it did not just read from the
  store.

## 4. Design

### 4.1 [AUDIT] The universal pre-POST gate — the core of the fix
This replaces v1's "double-checked load inside the lock". **Every** path that is
about to POST — locked, lock-timeout, and `invalid_grant` recovery — must:

```
reload the pair from the store
if the reloaded pair differs from the one we entered with:
        use it, DO NOT POST            <-- loser path: succeeds, zero POSTs
build the POST form FROM THE RELOADED PAIR
POST
```

Two rules that make it real:

- **Acceptance is "changed", not "fresh".** [AUDIT] Judging by freshness
  reintroduces clock skew and margin mismatch against `token_is_fresh`
  (`chatgpt.rs:392`): a loser could deem a perfectly good new token stale and POST
  a second rotation. *Reloaded pair ≠ entry pair → accept it, full stop.*
- **The form must be built after the reload.** [AUDIT] `chatgpt.rs:478-491` clones
  `refresh_token` into the single-flight closure **before anything runs**. Adding
  a reload without moving the form construction changes nothing — this is the
  easiest way to "implement the plan" and ship the defect.

### 4.2 Lock, scope, and ordering
Reuse `ExclusiveFileLock` (`wcore-config::credentials`, generalized from
`MigrationLock` by the splice fix). Scope: **load → decide → POST → store**.
Lock name from `SHA-256(service ‖ account)`, distinct from the store-write name.

**[AUDIT] Total lock order, stated as an invariant at every acquisition site:**
`in-process single-flight → refresh file lock → store file lock`. No exceptions.

**Nesting is certain, not forbidden.** v1 asked to "prove non-nesting" — wrong.
`refresh()` calls `storage.store()` (`chatgpt.rs:544`) which takes the store lock.
The requirement is a consistent *order*, not absence.

**The reverse order (store → refresh) is structurally impossible**, verified: every
`ExclusiveFileLock::acquire` call site is in `wcore-config/src/credentials.rs`, and
`wcore-config` does not depend on `wcore-agent`. The layering rule in `AGENTS.md`
("dependencies flow downward") forbids it. Record this as the reason, so a future
upward dependency is understood to break it.

### 4.3 [AUDIT] Timing, derived rather than inherited
- **Acquisition wait must exceed the maximum hold**: POST (20 s) + store-lock
  contention + write + scheduling. Derive both from `PER_CALL_TIMEOUT`; do not
  reuse MigrationLock's 10 s, which was sized for a sub-second migration.
- **Staleness must exceed the same maximum**, or a healthy holder is stolen
  mid-POST and the thief POSTs a concurrently-burned token.
- Add a heartbeat, or make staleness comfortably larger. State which and why.

### 4.4 [AUDIT v3] Failure policy — reversed TWICE
v1 said "proceed unlocked on timeout". v2 softened that to "reload, then POST as a
last resort". **The panel went 2.5-of-3 against even that**, and they are right:

- Gemini: hard local failure; never risk the grant.
- Codex: *"make POST-without-exclusive-refresh-ownership impossible — on
  contention, reload while waiting and return a winner's generation, otherwise
  **fail retryably**; never fall through to an unlocked POST, including after
  timeout or unsafe stale-steal."*
- Kimi: reload first, POST only as last resort.

**Decision — the asymmetry settles it.** An unlocked POST risks *grant revocation*:
unrecoverable without a full re-login. A retryable failure costs a retry. Those are
not comparable, so:

- **Lock timeout → reload (§4.1). If a new pair appeared, succeed.**
- **If none appeared, FAIL RETRYABLY. Never POST without the lock.** Not even as a
  last resort.
- The error must be explicitly retryable and say so, so a caller retries rather
  than treating it as an auth failure and prompting re-login.

### 4.5 [AUDIT] Async-safety
`MigrationLock::acquire` is a `std::thread::sleep` spin loop. Called from
`refresh().await` it blocks a tokio worker for the whole wait — with the wait now
sized above 20 s, that is a stall, and on a small executor a genuine
starvation deadlock (the blocked worker may be the one that would drive the POST
releasing the lock). Use `spawn_blocking` or an async acquire. **Not optional.**

### 4.6 [AUDIT] Compose the two failure policies deliberately
The store write **refuses** on lock contention (splice fix). Inside this critical
section, C4 (`chatgpt.rs:588-597`) hard-errors when rotation succeeded but persist
failed. Composed: POST succeeds → token burned server-side → store refuses →
winner errors → the store still holds the **burned** pair → every other process
reloads it and POSTs it. Two individually-correct policies produce exactly the
corruption each was written to prevent. **Resolve explicitly** — e.g. the store
write inside this section waits rather than refuses, since the refresh lock
already guarantees a single writer.

### 4.9 [AUDIT v3] Every token writer must join the protocol
**Codex, and nobody else caught it.** A store-write lock alone does not prevent
stale read-modify-write across *different operations*. `auth login`, `auth logout`
and the Codex import all write the token pair. An in-flight refresh that started
before a `logout` can complete after it and **resurrect the logged-out
credential** — the user believes they signed out and they are still signed in.
Enumerate every writer of the OAuth pair and make each take the same refresh lock,
or state per writer why it cannot race.

### 4.10 [AUDIT v3] `SingleFlightRefresh` is not cancellation-safe
**Codex.** If the primary future is dropped — a cancelled turn, a timeout upstream
— the slot at `flow.rs:640-660` stays occupied and the subscribers at `:682-687`
spin **forever**. This plan makes the primary hold *much* longer (file lock +
network + store), so it converts a narrow window into a routine one. Fix it here:
clear the slot on drop (a guard), and give the subscriber loop a bound.

### 4.7 In-process layer, and provider scope
Keep `SingleFlightRefresh` outside the file lock — cheaper for the common
many-tool-calls-one-process case. `chatgpt.rs` is the measured case; check `xai.rs`
and other `OAuthFlow` consumers and **state a verdict per provider** — if tokens
do not rotate, a duplicate POST is harmless and the lock is pure cost.

### 4.8 [AUDIT] Keep the in-process cache coherent
When the loser adopts a reloaded pair, the in-memory manager state must be updated
too, or the next call in that process uses the stale `refresh_token`.

## 5. Rejected alternatives
- **Lock only the POST** — both processes still decide to refresh; the loser burns
  a rotated token.
- **`invalid_grant` retry as the primary mechanism** — treats a predictable
  collision as exceptional. **[AUDIT] But keep it as *recovery*** (§6.4).
- **Hard-fail on lock timeout** (one auditor's preference) — protects the grant but
  converts contention into a refused turn where a reload would have succeeded.
  §4.4 achieves the protection without the refusal.
- **Shared refresh daemon** — correct, far too large for an RC.
- **Refresh inside the store lock** — holds a credential lock across a network call.

## 6. Proof obligations
1. **P1 red-then-green.** Two real processes, one profile, expired token, started
   together: exactly **one** POST at a local endpoint, both end working. Red
   control: revert the lock → **two** POSTs and an `invalid_grant`.
2. **P2.** Single process: one POST, no added latency.
3. **Loser-succeeds.** Assert the second process performed **zero** POSTs and still
   returned a valid token. [AUDIT] v1's proof #5 blessed the fallback without
   asserting it did not POST — a control that could not fail.
4. **[AUDIT] Crash between POST success and store commit.** Kill *after* the POST,
   before `store` lands: the rotated pair exists nowhere and the store holds a
   burned token. Assert the next process **recovers** (reload-and-recheck on
   `invalid_grant`) rather than dying. v1's "kill mid-POST" would have passed while
   auth was bricked.
5. **Dead holder.** Kill mid-POST; the next process proceeds after staleness.
6. **Lock unavailable.** Refresh still completes **via reload**, and asserts zero
   POSTs when a fresh pair was reloadable.
7. **[AUDIT] No executor starvation.** Contended acquire on a small multi-thread
   runtime must not stall unrelated turns. Measure.
8. **No deadlock** under the full suite, not in isolation.
9. **[AUDIT] Windows steal semantics** for a hold of tens of seconds, not the
   sub-second migration the primitive was sized for.
10. Mutant per property. A control that cannot fail is not a control.

**Local token endpoint only — never POST the real provider.**

## 7. Panel record
- **Gemini 3.1 Pro — REJECT.** Killed v1's proceed-unlocked policy on grant
  revocation; flagged cache coherence and clock skew. Its AB/BA deadlock concern
  was **checked and refuted** structurally (§4.2).
- **Kimi K3 — SOUND-WITH-FIXES.** Read the constants and found the 10 s-vs-20 s
  inversion, the closure-capture trap, the composed-failure corruption, the
  blocking spin on a tokio worker, and the crash window. Its "universal pre-POST
  gate" is now §4.1.
- **Internal adversarial** — found v1's own §3.1 error (nesting is certain, not
  forbidden) and verified the structural refutation of AB/BA.
- **Codex 5.6 Sol — SOUND-WITH-FIXES.** Landed on the third attempt (the first two
  hung on stdin — my invocation error). Contributed two findings nobody else had:
  §4.9 (logout resurrection — other token writers must join the protocol) and
  §4.10 (`SingleFlightRefresh` is not cancellation-safe, and this change makes it
  worse). It also correctly observed that the inspected tree has no
  `ExclusiveFileLock`, only `MigrationLock` — true at `plan/f20-…` because the
  splice fix is merged but **not yet promoted**. Verify against your actual base.
  **Full four-leg panel achieved.**

## 8. Out of scope
Non-rotating providers (pending §4.7); the daemon design; `flow.rs:682-687`'s
`yield_now` busy-poll for in-process subscribers — **[AUDIT] noted as made
materially worse** by longer primary waits, and worth its own item.
