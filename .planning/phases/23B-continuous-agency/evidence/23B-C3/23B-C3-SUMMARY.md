# 23B Criterion 3 — lane `23b-c3-memory` SUMMARY

Criterion text (verbatim, `23B-PHASE-VERDICT.md:19`):

> See and control memory/user-model activation, provenance, correction, forgetting, privacy,
> retention, nudges.

Verdict at lane start: **NOT MET**. Base `lane/grade-23b` @ `5bbb0fbc`, merged onto
`plan/f20-unified-audit-repair` @ `4a872413` mid-lane on the orchestrator's correction.
Lane HEAD `488dd9ff`.

**My grade: still NOT MET as a whole. Five of the seven verbs are closed for the memory
subsystem and proved at the outbound provider request body. Two are not: nudges is
half-closed, and the user-model half of the criterion is untouched.** Detail below, per verb.

---

## The finding that explains why C3 was NOT MET

Two disjoint retrieval paths existed, and every control was on the wrong one.

`AgentEngine::recall_relevant_facts` (`engine.rs`) runs on the first user turn of every session
and keeps **only `Partition::Semantic` hits** when it builds the `<system-reminder>` it injects
into the outbound provider request. So the semantic partition is the only memory that
automatically reaches a user's prompt.

Every control acted on `Partition::Episodic`:

| | auto-injected into the prompt | privacy enforced | retention enforced | provenance reported | `/memory forget` reaches | `/memory correct` reaches |
|---|---|---|---|---|---|---|
| episodic | no | YES | YES | YES | YES | YES |
| **semantic facts** | **YES, every cold turn** | **NO** | **NO** | **NO** | **NO** | **NO** |

`read_privacy_scope` and `read_retention` each had exactly **one** enforcement call site, both
hardcoded to `Partition::Episodic`. `search_with_provenance` omitted semantic hits *by deliberate
choice* — its comment argued that reporting a fused rank for a non-fused pass would be a
fabrication. The scruple was right; the conclusion was wrong, and it made `/memory why` silent
about the entire body of content that was actually in the prompt.

**The missing outbound-body proof is why nobody noticed.** A row-level test passes throughout all
of the above.

### Proved red at base, green after the fix

`evidence/23B-C3/base-redproof.log`, run on `hetzner-dsm` at base `5bbb0fbc` with the base control
API and the identical two-turn wiremock capture:

```
Summary 5 tests run: 1 passed, 4 failed
  PASS  the_probe_can_fail                                     <- harness alive
  FAIL  forget_at_base_leaves_the_value_in_the_outbound_body
  FAIL  privacy_at_base_reports_success_and_changes_nothing
  FAIL  retention_at_base_reports_success_and_changes_nothing
  FAIL  provenance_at_base_says_nothing_about_the_facts_in_the_prompt

BASE_FORGET_OUTCOME=Some("memory item not found: partition=episodic tier=project id=367a69b3-…")
BASE_PRIVACY_CONTROL_REPORTED=ok
BASE_RETENTION_CONTROL_REPORTED=ok
BASE_PROVENANCE hits=0 provenance_entries=0
```

Forget could not address the item at all. Privacy and retention **returned `ok` and changed
nothing about what was sent** — a control that reports success and does not act. Provenance
returned zero hits for a query the same harness proved does inject.

After the fix, at lane HEAD: `crates/wcore-agent/tests/memory_control_lifecycle.rs` —
**11 tests run, 11 passed** (`evidence/23B-C3/fixed-green.log` for the 8-test intermediate run;
final counts in `final-suites.log`).

---

## Per-verb ledger — all seven, graded separately

| # | Verb | Memory subsystem | User-model | Wire-proved | Live-proved |
|---|---|---|---|---|---|
| 1 | activation | **CLOSED** (was: no surface at all) | **NOT DONE** | yes | partial |
| 2 | provenance | **CLOSED** | **NOT DONE** | yes | yes |
| 3 | correction | **CLOSED** | **NOT DONE** (G3c) | yes | no |
| 4 | forgetting | **CLOSED** | n/a | yes | yes |
| 5 | privacy | **CLOSED** | **NOT DONE** | yes | yes |
| 6 | retention | **CLOSED** | **NOT DONE** | yes | report only |
| 7 | nudges | **HALF** — bound is reachable and settable; **nothing exists to bound** | n/a | unit | yes |

**1. activation — CLOSED for memory.** New `wcore-memory::activation` (`ActivationLog`,
`RecallActivation`, `ActivatedItem`), `MemoryApi::activation_log()`, and
`/memory activation [show|on|off]`. The engine honours the off switch **before any search runs**
and records what it injected **at the injection site**, so the record cannot drift from what was
sent. Three states are distinguishable and that is asserted: no recall has run / a recall injected
nothing / the user switched it off. The engine now takes `search_with_provenance` here so the
record can also report what a privacy scope or retention bound **withheld**.
*Stated limitation:* the record is per-process, so `wayland-core -p "/memory activation"` cannot
describe a previous process's turn. Within one process (the TUI) it can. Live-proved for state and
off switch; the record read-back is proved only by test.

**2. provenance — CLOSED for memory.** `facts_search_with_provenance` reports one honest
contributing modality (`Vector`, the cosine pass that really ran) rather than an invented fusion,
and the dispatcher merges it. `/memory why` now also prints **what each item says** — driving the
shipped binary showed it printed a uuid, a partition and a modality and never the text, which is
most of the question it exists to answer. Live output:
```
/memory why: 1 item(s) recalled
  #0 019fae21-…-7b93826abcbb [semantic/project] via vector score=0.65465 age=5s fresh
      the user recorded deployment region is QK7ZC3LIVE
```

**3. correction — CLOSED for memory.** `MemoryControls::correct_fact` **requires** the caller to
supply a re-embedding, and that is the design: `facts_cosine_pass` skips rows with a NULL
embedding, so keeping the old vector means a corrected fact is still recalled by the query that
matched the *wrong* text, and nulling it turns "correct" into a silent forget. `correct_recalled`
on the dispatcher does the re-embed from the triple read back off the row. Asserted at the wire in
both directions — new value present AND old value gone. **Not exercised by the live drive.**

**4. forgetting — CLOSED, and this is the one with legal weight.** `forget_fact` +
`forget_recalled`, falling through to the fact store only on `NotFound` so a gate denial stays a
denial. Proved at the wire in test, and **live on the shipped binary**, with the re-plant control
that makes the absence mean something.

**5. privacy — CLOSED.** Enforced in the semantic pass before any row is read. Proved at the wire
in test and live.

**6. retention — CLOSED.** Enforced in the semantic pass, and asserted **reversible** — relaxing
the bound returns the fact to the prompt, which is the only thing that makes "reported expired"
different from "silently destroyed". Live drive proves the report, not the wire effect.

**7. nudges — HALF, and I will not grade this closed.** The bound is now a control rather than a
constant: `cap`/`enabled` became atomics with `set_cap`/`set_enabled`, `MemoryApi::nudge_budget()`
exposes it, and `/memory nudge [show|on|off|cap N]` reaches it — live-proved. **But
`NudgeBudget::request()` still has no production caller.** There is no nudge delivery path in the
product; `provenance.rs` says so itself and defers it to Phase 24's persistent runtime. So a user
can now see and move a bound on something that does not yet happen. That is strictly better than a
constant nobody could reach, and it is not the criterion's "nudges" clause met.

**The user-model half of the criterion is untouched.** The criterion says
"memory/**user-model** activation, provenance, correction …". The verdict's G3c
("user-model correction precedence") is **NOT DONE**. The surface exists and is real
(`wcore-user-model`, `wcore-agent/src/user_context.rs`, `wcore-memory/src/partition/core.rs`),
and the gap is specific: `update_user_model` is `SystemToken`-only, and
`UserModelInferencer::infer` re-derives and overwrites at **every session end**, so a user
correction — if one existed — would be clobbered by the next inference. Nothing in this lane
changed that.

---

## Evidence

All on `hetzner-dsm`. Every count read back from an unproxied `cargo nextest ... --no-tests=fail
--retries 0`.

| Artifact | What it shows |
|---|---|
| `base-redproof.log` + `c3_redproof.rs.txt` | 4 FAIL / 1 PASS at base `5bbb0fbc` |
| `fixed-green.log` | 8/8 at the fix commit |
| `live-drive.log` + `c3-live-drive.sh` | **23/23 through the shipped binary** |
| `final-suites.log` | `wcore-memory` + `wcore-agent`: **3609 tests run, 3609 passed, 13 skipped** at `488dd9ff` |

The live drive runs the real `wayland-core` binary against a local endpoint speaking the Anthropic
SSE wire format, records every outbound request body, plants the fact **through the product** (a
real `assert_fact` tool call), and asserts on the captured bytes.

### Instrument discipline, and where it earned its keep

Three false results were caught by guards rather than published:

1. **A stale capture passed a nonce assertion.** A failed turn left an earlier request as "the
   latest capture" and the nonce check passed against the wrong body. Fixed by asserting **this
   turn's user message** in every body before asserting anything about the nonce.
2. **A vacuous `grep -F ""`** ("plant turn produced output") matched everything. Replaced with an
   assertion that the `assert_fact` tool really ran.
3. **The re-plant control caught a meaningless pass.** In one run the fact never reached the
   prompt at all (the approval gate had denied the tool), so the post-forget "absence" proved
   nothing. The control failed, the run was discarded, root cause found (`--auto-approve` needed
   in a scripted run), and only then was 23/23 recorded. **Without that control the run would have
   read 22/23 with the forget proof among the passes.**

Also: `/memory why`'s missing preview was found **only** by driving the binary live. No test would
have caught it — the data was correct and the rendering was not.

---

## What I did NOT do

- **G3c user-model correction precedence — not started.** Named above with the specific mechanism.
- **Nudge delivery** — deliberately not built. Shipping half a proactive actor to satisfy a
  criterion is the wrong trade; `provenance.rs` already says so.
- **`/memory correct` and the retention wire effect are not in the live drive** (both are proved
  at the wire by test).
- **macOS and Windows**: not driven. Linux only.
- No merge, no PR, no tag, no issue touched, no `wcore-contract generate`, no
  `wcore-cli/src/{lib,main}.rs` edit — **zero shared-file exposure**.
- `wcore-memory/src/db.rs` and the schema were **not** touched; lane `wal-nfs`'s journal-mode work
  is intact (`schema/mod.rs:55` still calls `sqlite_journal::configure`).

## Credential disclosure (lane brief §0)

The live drive needs the credentials vault unlocked — the binary itself said so. A **random
per-run passphrase** is generated on `hetzner-dsm`, written to a `chmod 600` scratch file inside
the disposable work dir, and passed via `WAYLAND_VAULT_PASSPHRASE_FD`. It is not a real
credential, it is not in any capture, log, commit or this summary, and it dies with the work dir.
No provider credential was used: the endpoint is a local mock and the API key is the literal
string `sk-live-not-real`.
