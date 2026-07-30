# 0003 — Durable sessions on a host with no secure credential store

Date: 2026-07-30
Status: **Accepted (degrade), with open items — supersedes the refusal posture of 2026-07-16**

> **Read this whole file before revisiting the question.** This decision has already been
> taken twice, in opposite directions, five days apart, because the second decision-maker was
> never shown the first. §7 states exactly what evidence a future revisit must have in front of
> it. If you are here because a test failed, start at §3.

---

## 1. The question

A host has **no confidential-capable credential store**: no usable OS keyring, and no unlocked
encrypted vault. Durable session persistence needs such a store, because it encrypts the exact
provider request into a crash-recovery journal so that journal never holds provider requests in
cleartext.

What should the product do?

- **REFUSE** — fail the turn with an actionable error; write nothing ambiguous.
- **DEGRADE** — turn durable sessions off, announce it, and run.

Both answers have been shipped. This file merges them.

---

## 2. Decision A — REFUSE (2026-07-16)

**Commit `906287e1790ab2e0c8a6f1f71940e9acc2b55c75`**, Thu Jul 16 12:54:28 2026 +0700,
`feat(recovery): seal interrupted turn state`.

Its stated reason, verbatim from the commit body:

> Make provider, tool, hook, budget, approval, and host recovery durable so restarts fail closed
> instead of replaying ambiguous effects.

The operative phrase is **"fail closed instead of replaying ambiguous effects."** The refusal was
not incidental to a persistence feature; it was the point. An interrupted turn whose provider
request was never recorded cannot be reconciled on restart, and the July-16 position was that
running in that state is worse than not running.

### The test that encodes it

`crates/wcore-cli/tests/f14_sigkill_recovery.rs:1106` —
`isolated_profile_without_secure_store_fails_before_turn_or_provider_intent`
(`#[cfg(target_os = "linux")]`, so it runs on Linux only).

It asserts, in order:

| line | assertion |
|---|---|
| :1119 | error code is `engine_error` |
| :1120 | `retryable == false` |
| :1124 | the message names the cause *and* both remedies — "secure recovery storage is unavailable", "OS keyring", "encrypted credentials vault" |
| :1133 | the turn terminates with `finish_reason == "error"` |
| :1135 | `fixture.observation().requests.is_empty()` — **the provider was never reached** |
| :1143 | `fs::read(&journal).expect(...)` — **the session journal file exists and is readable** |
| :1145 | no `turn_started` event in it |
| :1152 | the prompt text is not present in it |

**Note :1143 particularly.** `lane/durable-posture` caught this and it is load-bearing: the
July-16 posture was **not** "refuse and write nothing". It was **refuse, and journal the
refusal**. A durable, inspectable record that a turn was declined — with no turn started and no
prompt persisted. The degrade of §3 reverses *two* properties, not one.

---

## 3. Decision B — DEGRADE (2026-07-30)

**Commit `c73ac417df54ec9069a2c376d72feba218f8e85c`**, Thu Jul 30 20:20:56 2026 +0700,
`merge(fix-headless-keyring)`. Built by `d51287b1` (19:12) and `551d9001` (19:44).

### The evidence that motivated it — this part is not in dispute

> THE RELEASE BLOCKER. Every Linux server without an OS keyring could not complete a single
> turn, while gateway run started, channel probe said Ok and channel health said Healthy. It
> accepts work, reports healthy, answers nothing.

Found **independently by two UAT lanes approaching from opposite directions** — one via the
`[session] enabled = false` workaround (which removed the caller), one via
`WAYLAND_VAULT_PASSPHRASE` (which satisfied the callee). Both were partial: the lane then
falsified its own first answer and found `session_journal` has **three** production writers, and
writer #2 is the resume constructor reached by `AgentBootstrap::resume`, which
`channel_dispatch.rs:223-249` feeds with a live journal *regardless of the setting*. So the
documented `[session] enabled = false` workaround **did not cover resumed conversations** and
nobody had noticed, because every conversation the UAT lane tried was new.

That is a genuine, severe, correctly-diagnosed release blocker. Nothing in this ADR weakens it.

### The decision taken

At `Config::resolve_inner` (`crates/wcore-config/src/config.rs:2478`), upstream of every engine,
entrance and all three journal writers: if durable sessions are on, the backend *could* hold
confidential material, and no store is actually reachable — turn durable sessions off and record
why. Announced once on stderr at startup.

Deliberately narrow. Two neighbouring cases keep the hard refusal:

| situation | disposition | why |
|---|---|---|
| no keyring **and** no vault | **degrade** + notice | the host cannot do it; the operator did not ask for it |
| `backend = "plaintext"` | refuse (unchanged) | the operator configured a backend that can never hold confidential material |
| vault passphrase present but **wrong** | refuse (unchanged) | the store opens, so this surfaces as the distinct `SecureStoreUnreadable` |

### The panel

**DEGRADE 2 (codex `gpt-5.6-sol`, kimi K3) / REFUSE 1 (gemini 3.1 Pro).** All three were probed
alive before their votes were counted — gemini and kimi both returned 0 bytes with rc=0 on the
first attempt, and codex blocked on stdin needing `< /dev/null`. That instrument discipline was
correct and is not the problem here.

The stated reason for "no security property is weakened":

> with no journal there is nothing at rest for that encryption to protect

### What the panel was not shown

**Measured, with both instrument controls positive** (`/usr/bin/grep -rn` over
`.planning/evidence/fix-headless-keyring/` for `906287e1`, `f14_sigkill`, `sigkill`,
`seal interrupted`, `isolated_profile` → **zero hits**; the same grep for `keyring` in the same
files → 1 and 3 hits; the same grep for `sigkill` elsewhere under `.planning/evidence/` → 5 files.
The instrument was alive and the absence is real):

**No artifact of the fix-headless-keyring panel references Decision A, its reasoning, or its
test.**

And it is worse than an omission. The question actually put to the three legs
(`panel-question.txt`) reads:

> Proposed fix: at startup config resolution, if durable sessions are ON and no
> confidential-capable credential store is reachable, turn durable sessions OFF and print one
> stderr notice naming cause, consequence and remedy.
> **Rejected alternative: refuse to start at all.**

REFUSE was presented as a *rejected alternative* — not as **the incumbent behaviour, deliberately
chosen thirteen days earlier, with a passing test asserting it**. A panel told that one option is
already rejected is not being asked a neutral question. The 2-1 split has to be read in that
light.

---

## 4. The causation proof — measured on both sides

The claim "Decision B invalidated Decision A's test" is not inferred. It is measured.

**Ancestry** (`/usr/bin/git`, unproxied):

- `b8311575` is the **first parent** of `c73ac417` — i.e. integration immediately before the merge.
- `git diff --stat b8311575 c73ac417 -- crates/wcore-cli/tests/f14_sigkill_recovery.rs` is
  **empty**. The fix did not touch the test. Any behaviour difference is the product's.
- `c73ac417` is an ancestor of `e7bc6d88`, which is an ancestor of integration head `c9ab048b`.

**BEFORE** — `cargo test -p wcore-cli --test f14_sigkill_recovery` at `b8311575`, run on
`hetzner-dsm` in a fresh worktree, 672 crates compiled from source, binary self-reporting its HEAD
(`.planning/evidence/decision-record/f14-at-b8311575-BEFORE.log`):

```
running 12 tests
test isolated_profile_without_secure_store_fails_before_turn_or_provider_intent ... ok
test result: ok. 11 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 14.54s
```

**AFTER** — the same test binary at `e7bc6d88` (fix present), measured by `lane/fix-tui-noise`,
which was investigating something unrelated and had no stake in this question
(`.planning/evidence/decision-record/f14-at-e7bc6d88-AFTER-solo.log`):

```
test result: FAILED. 10 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out
```

```
panicked at crates/wcore-cli/tests/f14_sigkill_recovery.rs:204:9:
assertion `left == right` failed: packaged Core opened a different session
  left: None
 right: Some("f1400000000000000000000000000000")
```

Same 12 tests, 1 ignored on both sides, 0 filtered out on both sides, **exactly one test flipped**.
The test-binary metadata hash is `0c2c7646b8296b33` in both logs, consistent with a byte-identical
test file. `lane/fix-tui-noise` went further than it needed to and proved the red was not its own:
it reverted both its source files to base, confirmed both crates actually recompiled, and re-ran
under `RUST_LOG=info` (which disables its new code path entirely) — identical failure.

**Causation established.**

### Where it dies is itself a finding

Line 204 is **not** the journal read. It is the `ready`-handshake assertion inside
`CoreProcess::launch*`. The test dies at process launch, long before any journal assertion —
because the `ready` frame **no longer carries a `session_id` key at all**.

Commit `c73ac417` lists under "NOT DONE, and not claimed":

> `--json-stream` and Desktop hosts are NOT told: the notice goes to stderr and the decision
> precedes any protocol writer.

That is **understated**. Desktop hosts are not merely un-told. They are told something wrong,
silently: the `ready` payload in the capture contains `capabilities`, `contract`,
`execution_policy`, `type`, `version` — and no `session_id`. A host reading `ready.session_id`
gets `undefined`, with no explanation anywhere in the protocol stream. The degraded mode **is**
observable over the wire; it is observable as a *missing field* rather than a declared state.
That is the worst of both options — an unannounced contract change.
`durable_sessions_disabled_by_host()` (`config.rs:2598`) exists for exactly this and still has no
consumer.

---

## 5. The second panel — the refutation of "nothing to replay"

A second panel was convened the same day with **both** decisions in view. Verdicts:
**SOUND-WITH-CHANGES, SOUND-WITH-CHANGES, UNSOUND.** It refuted the argument for keeping the
change as stated.

> **Provenance caveat, stated because this ADR exists to stop exactly this.** I could not locate a
> committed transcript of the second panel anywhere in the repository
> (`/usr/bin/grep -rl` over all `*.md` for its distinguishing phrases → zero; the only
> `UNSOUND` hit in the tree is `.planning/phases/20-…/archive/native-split/20-71-PLAN.md`, which
> contains **0** occurrences of `keyring` or `durable session` and is unrelated). Its content
> below is as relayed in the lane brief, **not** independently verified against a transcript.
> The refutation reproduced here is recorded because it is *correct on the merits* — the
> reasoning stands on its own and can be checked by reading the code — not because I confirmed
> the panel ran. **A future revisit should treat §5 as argument, and §2/§3/§4 as measurement.**

### The core correction

Decision B's security argument is: *with no journal there is nothing at rest for that encryption
to protect.* True — and irrelevant to what Decision A was actually protecting against.

Decision A's concern was **"replaying ambiguous effects."** The refutation:

> Retries do not come from the engine. They come from the platform redelivering an inbound, an
> operator retrying, or a human resending. So a missing record does not remove ambiguity — it
> **exports it to the user.**

This is the load-bearing correction. "Nothing to replay" is only true if the engine is the sole
source of retries. It is not. Under DEGRADE, a crash mid-turn leaves:

- the side effects already committed (tool writes, sends, spends) — **still real**;
- no durable record that they happened;
- an inbound the platform will redeliver, or a human who will resend because they got no answer.

The ambiguity did not disappear. It moved from a place the product could reason about to a place
only the user can, and the user has strictly less information than the journal had. Decision B
correctly observed that *confidentiality at rest* is monotonically better under degrade. It then
generalised that to "no security property is weakened", which does not follow: **integrity of
effect accounting is not a confidentiality property**, and that is the one Decision A was
defending.

`lane/effect-accounting` is measuring the two concrete consequences (§6).

---

## 6. The false dichotomy — identified independently by two legs

**"No secure store ⇒ no journal" was never the only option.**

The choice was framed as *encrypt the journal, or do not journal*. But the thing that is
unavailable is the **encryption key**, not the ability to write a file. The available move is to
**degrade the ENCRYPTION, not the JOURNALING** — journal at a reduced confidentiality level
(redacted provider payloads, or a machine-local key, or metadata-only frames), keeping effect
accounting and crash reconciliation, and announce *that* posture.

This is not speculative. `lane/durable-posture` localised the entire cliff to one predicate
(`crates/wcore-config/src/credentials.rs:1104`):

```
vault_unlock_material_present()  =  WAYLAND_VAULT_PASSPHRASE_FD is set  ||  WAYLAND_VAULT_PASSPHRASE is set
```

`CredentialsBackend::Auto` offers exactly two confidential candidates — `Keyring{service}` and
`EncryptedFile{cipher,kdf}` — and the `EncryptedFile` candidate is judged unavailable *purely
because nobody typed a passphrase*. There is no third candidate and no auto-provisioned key. The
binary cliff is a consequence of that one function having no middle, not of any property of the
host.

Note also the existing precedent for the stricter half: a backend marker is pinned at
`.credentials.confidential-backend.json` on first successful open, and a pinned backend that later
goes unavailable is a **hard error, not a downgrade** (`credentials.rs:506-510`). The mechanism for
"refuse when a journal exists but its key does not" already exists at the credentials layer.

---

## 7. Current resolution, and what a future revisit MUST have in front of it

### Resolution

**DEGRADE stands as shipped.** The release blocker is real, was independently reproduced twice,
and REFUSE cannot fix it by construction — refuse-to-start makes the default install on every
Linux server dead until the operator discovers an environment variable. That is not a defensible
default.

But it stands **as an interim posture, not as the answer**, because §5 and §6 show the reasoning
that justified it was too broad and the option space was never fully explored.

### Open items

| # | item | owner |
|---|---|---|
| 1 | Journal at reduced confidentiality instead of not at all (§6) — dissolve the one-predicate cliff | `lane/durable-posture` |
| 2 | Measure what is actually lost: budget spend unjournaled (a crash-looping daemon re-arms its ceiling every restart and bills forever); approval state evaporating (a mid-approval crash loses the human's answer, and a destructive op may be re-asked and re-approved) | `lane/effect-accounting` |
| 3 | `durable_sessions_disabled_by_host()` has **no consumer** — the degraded state is printed, not reportable | `lane/fix-channel-health-truth` |
| 4 | The `ready` frame silently drops `session_id` (§4) — an unannounced protocol contract change | unassigned; needs a seam request |
| 5 | `switch_active_session`, journal writer #3, is still unguarded | unassigned |
| 6 | The July-16 test is red in integration and is **not** stale — it encodes a live disagreement. Do not delete it. Re-point it once item 1 lands | `lane/durable-posture` |

### The revisit bar

**This decision has flipped once already because a decision-maker saw one side. Do not convene a
panel on this question without putting all of the following in front of it — in the question
itself, not in a reference:**

1. **§2 in full** — Decision A, its verbatim reason, and the fact that its test asserts *both* no
   provider contact *and* a readable journal.
2. **§3 in full** — the release-blocker evidence. Two independent UAT lanes; the three journal
   writers; the fact that the documented workaround did not cover resumed conversations. Any
   panel shown only §2 will vote REFUSE and re-break every headless Linux install.
3. **§4** — that these two are in direct, measured conflict, with the before/after counts.
4. **§5 and §6** — that "nothing to replay" is refuted, and that the dichotomy is false.
5. **The framing must be neutral.** Do not present either option as "the rejected alternative".
   That is the specific defect that produced the 2-1 split in §3.

**A vote taken without items 1-5 does not count, and this file is the reason.** If you are
writing a panel question about durable sessions, credential availability, headless operation, or
the recovery journal, link this ADR and paste §2 and §3 inline.

---

## 8. Why the process failed, not just the decision

The two decisions are each defensible on the evidence their author had. Neither author did
anything careless. Decision B's lane ran a live four-quadrant proof, falsified its own first
answer, probed every panel leg alive before counting its vote, conceded a rider to the dissent
and built for it, and listed five things it had **not** proven. That is exemplary work. It still
produced an oscillation, because the one thing it could not do was know that Decision A existed.

That is a **discovery** failure, not a diligence failure, and no amount of care by an individual
lane fixes it. The proposal is in
[`docs/decisions/0004-cross-audit-panels-must-see-prior-decisions.md`](0004-cross-audit-panels-must-see-prior-decisions.md).

Two structural gaps found the same day compound it, and both are in that file:

- The merge cadence gates on `fmt` + `metadata --locked` + `check --workspace --all-targets`, and
  runs **no tests and no clippy**. That is precisely why a behaviour change that invalidated an
  existing test reached the integration branch unnoticed.
- The only surviving artifact of Decision A's reasoning was a test — and a red test with no
  attached reasoning reads as stale. It nearly got deleted as such. §9 fixes that.

## 9. Findability

A pointer to this ADR is attached to `RecoveryConfidentialError` in
`crates/wcore-agent/src/recovery_confidential.rs` — the six-variant error enum written
specifically so that these causes do not collapse into one string. That enum is what a reader
reaches when they hit "secure recovery storage is unavailable" and start asking why, and it is
not owned by any single decision, so it survives both.
