# 23B Criterion 3 — **user-model half** — lane `23b-c3-usermodel` SUMMARY

Criterion text (verbatim, `23B-PHASE-VERDICT.md:19`):

> See and control memory/**user-model** activation, provenance, correction, forgetting,
> privacy, retention, nudges.

Base `plan/f20-unified-audit-repair` @ `eaff921d`. Integration merged mid-lane
(`gh/plan/f20-unified-audit-repair` @ `632ad619`), clean, no conflicts.

**My grade: criterion 3 is still NOT MET.** On the user-model half, **1 of 7 verbs is
closed**, 3 are half, 3 are not met. I graded per verb and not as a whole, because the
aggregation failure is the one this programme has paid most for.

---

## The finding that changes what G3c means

**There are two disjoint "user model" surfaces, and the one G3c names is the one that
never reaches the model.**

| | surface | mutation verbs at base | reaches the outbound prompt? |
|---|---|---|---|
| **A** | `wcore-memory` P5 `user_model` k/v — `MemoryApi::update_user_model` | `update_user_model`, `AccessToken::System` only | **NO** |
| **B** | `wcore-user-model` `UserBrief` + `Preferences` → `render_user_context_block` | **`observe` only** — an EMA inference fold. No correction verb existed at all. | **YES**, `bootstrap.rs` |

This is the *identical shape* to the defect the memory half closed today — controls acting
on `Partition::Episodic` while only `Partition::Semantic` reached the prompt — and a
literal reading of G3c would have walked straight back into it. **Fixing only A would have
produced a correction that survives forever and is never seen by the model.**

### Proved at base, not asserted

`base-red.log`, run at `eaff921d` on `hetzner-dsm`, drives a correction through the only
surface base offers (`update_user_model`, G3c's named surface) and reads the wire:

```
test base_probe_is_alive ... ok            <- harness alive; inferred model IS on the wire
test base_user_model_correction_never_reaches_the_provider ... FAILED

BASE_CORRECTION_WRITE_REPORTED=true
BASE_CORRECTION_IN_STORE_AFTER_SESSION_END=Some("\"blunt, no preamble QK7UM3NONCE\"")
BASE_CORRECTION_ON_WIRE=false   BASE_INFERENCE_STILL_ON_WIRE=true
```

The write is accepted, the value is still in the store after session end — **and it never
reaches the model, while the belief it was meant to correct still does.**

### G3c's clobber claim, measured exactly (`clobber-scope.log`)

G3c says `UserModelInferencer::infer` "overwrites at every session end". Measured at base:

```
INFERENCER_KEYS=["preferences.tool_order","tool_habits.recent_top5",
                 "language.primary","working_hours.local_tz_window"]
OWNED_KEY_AFTER_SESSION_END=Some(String("en"))          (user wrote "ja-USERSAID")
UNOWNED_KEY_AFTER_SESSION_END=Some(String("blunt-USERSAID"))
```

**The clobber is real but scoped to the four keys the inferencer derives.** A correction to
`language.primary` is silently reverted; one to any other key survives. So G3c is right in
mechanism and wrong in reach — and both cases are moot for the user, because
`update_user_model` has **no user-reachable and no model-reachable caller at all** (zero
hits across `wcore-tools`, `wcore-cli`, `wcore-skills`, `wcore-protocol`; instrument proved
alive in the same search by a known-positive on `assert_fact`, 3 hits).

**Therefore I did not add precedence to surface A.** Adding an origin column and a
migration to a partition no user can write and no prompt can read would be ceremony. I
fixed the surface that reaches the model instead, and report A precisely rather than
"fixed".

---

## What I built

A **user-authored correction layer that inference cannot reach** — not by convention, but
structurally: `CorrectionStore` is a different type, in a different file, persisted to a
different JSON document (`user-corrections.json`, never the `user-model.json` that the
EMA fold rewrites wholesale), and it is **not reachable from `UserModelBackend::observe`
at all**.

Precedence is **subtractive**: a corrected subject's inferred line is *removed* from the
system prompt, not printed next to the correction. Printing both hands the model a
contradiction and lets it choose, which is a coin flip the user cannot see.

Surface: **`/usermodel show | correct <key> <value> | forget <key>`**. Registered only when
a real store was opened — no `Stub` variant, so the command either works or is absent.

`/usermodel show` also displays **what the agent inferred**, each line marked
`[inferred — sent to the model]` or `[OVERRIDDEN by your correction — not sent to the
model]`. I added this after grading: corrections alone were not "see and control", because
the product was asking users to correct beliefs it never showed them. An overridden
inference stays visible, because it is exactly what `/usermodel forget` restores.

---

## Per-verb ledger — the user-model half, all seven, graded separately

| # | Verb | Grade | Basis |
|---|---|---|---|
| 1 | activation | **HALF** | *See*: yes — `/usermodel show` lists every inferred belief and says which reach the model. *Control*: **no** — there is no off switch for the user-context block. |
| 2 | provenance | **HALF** | Origin class is reported per line (inferred vs user-stated vs overridden); dialectic inferences carry native confidence + observation count. There is **no per-turn evidence trail** for style/expertise/tags. |
| 3 | correction | **CLOSED** | Wire-proved (3 tests) **and** live-proved (19/19 on the shipped binary). Survives session end, 25 EMA folds, and the P5 session-end inference. |
| 4 | forgetting | **HALF** | A user can drop their own correction and the subject provably returns to inference on the wire. A user **cannot erase an inference** — nothing clears the brief. |
| 5 | privacy | **NOT MET** | No scope, no redaction, no way to stop the user-model block reaching the provider. Nothing built. |
| 6 | retention | **NOT MET** | No age bound on user-model data. `last_observed_ts` is recorded and nothing ever expires. |
| 7 | nudges | **NOT MET (deferred, and now honestly so)** | See below. |

**1 CLOSED, 3 HALF, 3 NOT MET → criterion 3 remains NOT MET.**

---

## The nudges decision: de-advertised

`NudgeBudget` is fully implemented and settable, and **`request()` has no production
caller anywhere** — there is no nudge delivery path in the product. A user could move a
bound on an event that never fires.

Cross-model panel, **unanimous 3/3 for (b) — stop advertising it** (`panel-codex.txt`,
`panel-gemini.txt`, `panel-kimi.txt`, votes extracted unanimously with a last-match
unanchored regex per the brief). Codex: *"(c) is honest at the sentence level but still
creates the forbidden third state at the product level."* Kimi: *"a permanently
self-disclaiming command is zombie UI."* My internal adversarial pass argued the opposite —
that removing a surface the previous lane shipped and live-proved today is churn — and I
rejected it: the command is three lines to restore, the infrastructure and its concurrency
tests stay, and the alternative is an advertised control governing nothing.

**Done:** `/memory nudge` removed from dispatch and help. `NudgeBudget` retained untouched
for the phase that ships delivery. Verified live, paired with a known-positive so the
removal cannot be confused with a dead binary:

```
UM_LIVE=PASS  KNOWN-POSITIVE: /memory still dispatches its real sub-actions (found 'automatic recall')
UM_LIVE=PASS  /memory nudge no longer exists (found 'unknown sub-action')
```

I graded nudges **NOT MET**, not HALF. Deferral stated plainly is not redefining success.

---

## Evidence — every number read back from an unproxied tool

| Artifact | Shows |
|---|---|
| `base-red.log` + `base_redproof.rs.txt` | **base RED**: correction not on wire, inference still on wire |
| `clobber-scope.log` + `clobber_scope.rs.txt` | the clobber is real, and scoped to 4 keys |
| `fixed-wire.log` | **3 passed, 0 failed, 0 ignored, 0 filtered out** |
| `usermodel-live-drive.log` + `.sh` | **19/19 GREEN** through the shipped binary |
| `memory-correct-live-drive.log` + `.sh` | **11/11 GREEN** — memory-half `/memory correct` |
| `final-suites-and-show.log` | **3711 passed, 0 failed, 8 skipped**; live `/usermodel show` output |
| `credential-sweep.log` | live-key hits **0** |

The wire proof (`crates/wcore-agent/tests/user_model_correction_wire.rs`) correct**s**,
ends the session, runs *everything G3c says would clobber it* (25 EMA folds + the P5
session-end inference), starts a new session, and asserts on
`wiremock::received_requests()` — the literal bytes that would have gone to Anthropic.

### The known-negative that makes the absence mean something

`the_control_proves_the_probe_can_fail` runs the identical two-session shape with **no
correction** and asserts the inferred line **is still on the wire**. Without it, EMA drift
or a bootstrap reorder would remove the string on its own and the headline test would pass
having proved nothing.

### Two defects in MY OWN instrument, repaired in-lane

1. **An exact-value marker was self-passing.** `INFERRED_STYLE_MARKER` was
   `formality=0.40`. `observe_user_turn` folds a fresh fingerprint every turn, so the
   number **drifts between sessions** — the marker would have gone absent by itself and
   the headline assertion would have passed with the correction doing nothing. Caught only
   because the sibling forget test went red for exactly that reason. Now structural
   (`- style: formality=`), and the control asserts that same string is present.
2. **A non-hex `--session-id` killed an entire drive.** `c3um1fee…`/`c3mc1fee…` contain
   `u`/`m`; the binary refuses. 17 of 19 checks failed — **and one absence check PASSED on
   a non-empty file**, the self-passing class in miniature. Repaired in both scripts, with
   the constraint commented at the line (brief §6b-ii: fix the instrument, do not merely
   note it).

---

## Memory-half items I took

- **`correction` live-driven** (was: wire-test coverage only). `memory-correct-live-drive.sh`,
  **11/11**. It plants a fact through a real `assert_fact` tool call, proves the original
  text on the wire, corrects it through `/memory correct`, and proves the new text arrives
  and the superseded text is gone.
- **This found a live-only product defect, now fixed.** `/memory correct` printed
  `<uuid> corrected in semantic/project` and **never the corrected text** — the identical
  class the memory lane found in `/memory why`, where the data was right and the rendering
  was not. No test would ever have caught it. It now prints what the item says.
- **`retention` enforcement — NOT taken.** Still proved by report only, not by wire effect.

---

## What I did NOT do

- **No fix to surface A (P5 `user_model`).** Deliberate, with evidence above: no user path,
  no model path, no prompt read. Reported precisely rather than papered over.
- **privacy and retention on the user-model half: nothing built.** Graded NOT MET.
- **No activation off-switch** for the user-context block, and **no way to erase an
  inference** (only to override or un-override one). Both graded honestly as HALF.
- **No nudge delivery path.** Deliberate; de-advertised instead.
- **Linux only.** Not driven on macOS or Windows.
- **`UserModelBackend` trait unchanged** — corrections deliberately sit outside it, so the
  Honcho backend gets them for free and there is no unsupported path.
- No merge to integration, no PR, no tag, no issue touched, no `wcore-contract generate`.
- `wcore-memory/src/db.rs`, `sqlite_journal.rs` and the schema **untouched** — lane
  `wal-nfs`'s journal-mode work is intact.

## Shared-file exposure (brief §6)

`crates/wcore-cli/src/main.rs` only, **additive, one contiguous block** inside
`build_slash_dispatcher` (registers `/usermodel`, with the inference backend attached, when
a store exists). Diffed against the captured merge-base `eaff921d`, not the branch name:

```
$ git diff --stat eaff921d HEAD -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
 crates/wcore-cli/src/main.rs | 13 ++++++++++++-
 1 file changed, 12 insertions(+), 1 deletion(-)
```

`crates/wcore-cli/src/lib.rs` untouched.

## Credential disclosure (brief §0)

No provider credential was used. The live drives override `HOME`, so
`/root/.wayland/.env` is not loaded — confirmed by the product's own output
(`vision: no API key found (ANTHROPIC_API_KEY …)`), read back rather than assumed per
§3b-ii. The endpoint is a local mock and the API key is the literal `sk-live-not-real`;
traffic reaching the mock is proved by the mock's own captures. A random per-run vault
passphrase is generated on the host, written `chmod 600` inside the disposable work dir,
and passed via `WAYLAND_VAULT_PASSPHRASE_FD`.

Sweep (`credential-sweep.log`), with a liveness control in the same invocation:

```
SWEEP_KEY_LEN=108  (value never printed)
SWEEP_INSTRUMENT_LIVENESS=20 files matched a known-present nonce (must be >0)
SWEEP_LIVE_KEY_HITS=0 (expected 0)
SWEEP_ANY_REAL_KEY_SHAPED_HITS=14 (expected 0)
```

The 14 are **all pre-existing placeholders** in files not in my diff
(`sk-ant-api03-xxxx…`, `sk-ant-not-a-real-key-000…`, `sk-ant-harness-not-real-key-…`,
`sk-ant-smoke-not-a-real-key-…`); my regex was simply broader than the real-key shape.
The live key itself: **0 hits**.
