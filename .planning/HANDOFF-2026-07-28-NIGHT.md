# HANDOFF — Wayland Core, 2026-07-28 night

Integration `plan/f20-unified-audit-repair` @ **`0b16f867`**. **20 lanes merged today.**
Supersedes `HANDOFF-2026-07-28.md` where they disagree; that file's **§0 rules, §3 traps and
§4 do-not-disturb still bind** and are not repeated here.

---

## 0. Sean's live concern, answered with measurement

He asked, tonight: *"I'm very concerned that the parity has not been met and a lot of these
others have not been completed."*

**The honest answer has two halves, and they point opposite ways.**

**Half one — the record understates us, measurably.** Phase 30-01's independent ledger review
(`30-01-LEDGER-REVIEW.md`) found CTRL-01's schema sound — 10/10 families pass all seven clauses,
both peer baselines re-verify exactly — but **13 claims falsified by the tree, and EVERY ONE
UNDERSTATES the program.** Its own headline: *"do not position from CTRL-01's Limitation columns
as written."* `PORT-*` still says the import half is unbuilt (26-02 and 26-04 are complete);
`REACH-*` still lists two Sean-reserved blockers that were cleared today. So the parity document
is **pessimistic**, not optimistic. That is the rarer and safer direction, and it is why 30-02
exists.

**Half two — two families are genuinely behind, and no amount of ledger hygiene changes it.**
`MEDIA-*` has not moved at all: Phase 27 graded **0 of 5 requirements complete**, no audio ever
flowed, no generation shape was ever exercised. `GATEWAY-*` made the largest single move in the
program (ABSENT → CONSTRUCTED, three levels) and is **still the widest measured gap** — both
peers ship full gateway stacks. Those are real.

**The distinction that matters most for his decision, and it should be put to him plainly:
parity and shippability are different targets.** Full parity across ten families against two
mature competitors is a multi-month goal. A defensible release candidate is not. Conflating them
is the single likeliest way to spend another two weeks. Phase 30 exists precisely to tell him
what he can honestly claim — it is not a gate on shipping.

---

## 1. What closed today

**Phases complete:** 25 (all four criteria MET), 26 (SC3 open — `migrate` has no rollback), 28
(certified; **C4 NOT MET, gate deliberately not passed**), 29 (all four PARTIAL, honest).

**Five of six release blockers closed** — see `.planning/RC-READINESS.md`, which is current.
Remaining: **`24-C3`**, the inbound channel matrix, NOT MET.

**Four defects that would have shipped in the RC**, each found by driving the real product on
real hardware, none visible from source review, two actively masked by gates that passed:

1. **Remote command injection**, root execution on the far end (`SECURITY-NOTE-SSH-INJECTION.md`).
   Never shipped — no tag contains `d0fc5095`, instrument verified against 36 tags.
2. **Data loss on interrupted migration** — 331 payload directories orphaned, 0 profiles
   imported, on 5 of 35 kills.
3. **The cloud backend was broken, not merely unexercised** — two of three defects produced
   *false greens*.
4. **`--trigger poll:` fired unconditionally without ever contacting its URL.**

---

## 2. The overnight wave — six dispatched, six merged

| Lane | Doing |
|---|---|
| ~~`lane/30-02`~~ | **MERGED.** Wayland loses two comparatives — **but the protocol that produced them is defective and the lane proved it by running it** (see §2b) |
| ~~`lane/24-c3-h2`~~ | **MERGED.** Gateway hosts inbound; `F24-C3-H4` spun out |
| ~~`lane/27-gaps`~~ | **MERGED.** Goal still NOT ACHIEVED and the lane says so — but audio flowed for the first time, and the credential-free path was made real |
| ~~`lane/22-c3`~~ | **MERGED.** Criterion 3 **PARTIAL** — the first honest grade in four passes. Enforced over the Goal lifecycle, convention outside it, and it says which is which |
| ~~`lane/29-h1`~~ | **MERGED.** `F29-02-H1` closed at source; `ignore = []`, both vulnerable versions out of the lock |
| ~~`lane/28-h2`~~ | **MERGED, FIXED.** Stale-lease DoS reclaimed and quarantined. Ledger row still reads `OPEN` — the lane refused to mark its own homework, so `lane/28-adj` adjudicates independently |

**Now running:** `lane/30-03` (positioning — the call 30-01 and 30-02 both declined),
`lane/28-adj` (independent adjudication), `lane/24-c3-h4`, `lane/headless-keyring`.

### 2a. I grounded both HIGH lanes before dispatch, and got two things wrong. Both matter.

I wrote the grounding below into the dispatch briefs as fact. The lanes disproved both, **against
explicit instructions from me not to**, and were right to.

- **`F29-02-H1` — I told the lane the calamine leg was withdrawn as wrong and not to resurrect it.**
  It re-measured instead. Both advisories declare `patched = [">= 0.41.0"]` with **no `unaffected`
  range**, so quick-xml 0.31.0 is in scope after all, and `cargo audit` with the ignore list
  bypassed reports **4 vulnerabilities, not 2**. I had relayed a withdrawal nobody had checked
  against the advisory metadata.
- **`F-28-02-002` — I claimed no expiry, reclamation or owner-liveness existed, having grepped for
  the word `stale`.** All of it existed under other names: `owner_pid`, `owner_creation_time`,
  `owner_is_live()`, `recover_dead_leases_locked()`. **A keyword grep is not a concept measurement**,
  and the brief I wrote pointed at the wrong repair. The real defect was two `return Err` aborting
  the whole recovery pass forever — and `storage.rs:59` **already named it in its own words**
  ("there is no quarantine path"). Someone documented the defect and nobody acted on it.

**The generalisable rule: a brief's grounding is a claim, not a premise.** Both lanes produced the
right answer by treating my instructions as falsifiable. Write briefs that make that explicit.

---

## 3. Open HIGHs

| ID | What |
|---|---|
| ~~`F24-C3-H2`~~ | **CLOSED, merged.** The gateway hosts inbound, and refuses at startup naming the cause when it cannot. Live-proven on Linux; **Criterion 3 still NOT MET** and the lane declined to record it as closed. |
| ~~`F24-C3-H4`~~ | **REPRODUCED, RAISED TO HIGH, FIXED, merged.** The race was real and worse than filed — the subscriber-less manager swept and confirmed the queue **3 ms before the second manager had registered**. Startup: 8 of 8 lost → 0. **Severity came from the steady-state legs**: after a 45 s settle, 5 of 6 lost, with a control ruling out "the adapter just stops" — so the loss is **ongoing, silent, and produces no error**. Email IMAP and Discord share the mechanism and are **unmeasured**. |
| `F-28-02-002` | Stale AppContainer lease = DoS. **OPEN at HIGH by choice** — 28-04 declined a MEDIUM re-score that a literal reading permitted, because the downgrade opens the accept path and passes the gate. |
| ~~`F29-02-H1`~~ | **CLOSED AT SOURCE, merged.** Suppression removed entirely (`ignore = []`), both vulnerable quick-xml versions gone from the lock. |
| `F29-03-01` | `self-update` installs nothing until a trust root + manifest asset exist. Fail-closed by design. |

---

## 4. New traps measured today — all cost a real run

- **`<RestartOnFailure>` registers, reads back through Task Scheduler's own `/query /xml`, and
  DOES NOT WORK.** Service stayed dead 3m20s. **A gate asserting its presence would have
  certified a service that never comes back.** Use `<TimeTrigger>` + `<Repetition>` +
  `MultipleInstancesPolicy=IgnoreNew`.
- **`encoding="UTF-8"` is rejected** by Task Scheduler; UTF-16-declared UTF-8 bytes are accepted.
- **`%USERDOMAIN%\%USERNAME%` is rejected on a workgroup machine** — emitting a `<Principals>`
  block breaks install on **every non-domain-joined desktop**.
- **A green can be manufactured by universal denial.** 24-C3's `access` leg passed on all three
  adapters at the pre-fix binary *because everything was denied*.
- **Four flavours of "reports success having run zero tests"**: all `#[ignore]`d; env-gated early
  `return`; **a filter matching no test name**; a file-level `#![cfg(feature=…)]`. Reading
  `N passed` back is necessary but **not sufficient** — one suite prints `8 passed` from a support
  module while running neither real case. **Run targets by file, never by filter.**
- **Live Windows requires `--test-threads=1`** — parallel gave 3/2, 2/3, 1/4 at one commit;
  serial a flat 4/1 over 12.
- **A silent poll loop is indistinguishable from a hung agent** and the watchdog kills it. Emit
  every iteration, bound the loop, commit before any long wait. It killed six lanes today.
- **The instrument that hunts a defect class tends to carry it — now TEN times.** Run every
  checker against a known-positive *and* a known-negative before trusting it. Two more overnight:
  a `--list` regex anchoring `$` against trailing CRs reported zero resolved tests, and **MarkdownV2
  escaping mangled a correlation token so a run with eight successful replies printed `replied=0`**
  — one step from writing up a working path as total loss. The fix there is the transferable part:
  an explicit `instrument_fault` state that grades such a run **INCOMPLETE rather than LOSS**.
  Build that state into any instrument whose failure mode looks like the defect it hunts.
- **A WITHDRAWN finding leg is not a settled one.** `F29-02-H1`'s calamine leg was withdrawn as
  "wrong" because quick-xml 0.31.0 is "not named by either advisory". Nobody checked the advisory
  metadata: both declare `patched = [">= 0.41.0"]` with **no `unaffected` range**, so every
  version below 0.41.0 is in scope. `cargo audit` with the ignore list bypassed reports **4
  vulnerabilities, not 2**. The withdrawal made a HIGH look smaller than it was, and it was
  carried forward into a lane brief unchallenged. **Re-measure a withdrawal the same way you
  re-measure a finding** — a correction is a claim too. The lane that caught this did so by
  refusing an explicit instruction in its brief and showing the tool output instead; that is the
  behaviour to want.

---

## 4b. Voice ships in no artifact — and that is NOT the advertised-but-dead class

`27-gaps` found `voice_mode` behind `#[cfg(feature = "voice")]`, present in neither `wcore-cli`'s
default features nor `release.yml`. **No shipped artifact contains voice at all**, confirmed live —
the default build exits 2.

**I checked whether this is the false-advertising class before escalating it, and it is not.**
`grep -rn -i "voice mode\|voice_mode" README.md docs/*.md` returns **zero matches**; `Cargo.toml:56`
documents it as an opt-in `cargo build -p wcore-cli --features voice`. The README's only voice
claim is inbound voice-note transcription, and it already discloses that as inert without a key.
So nothing user-visible promises a voice mode that isn't there.

**Therefore it is NOT added to the release blockers**, on the same reasoning that keeps the 11
can-ship-open criteria off that list. What it is: Phase 27 Criterion 4 **cannot be met by any
shipped artifact**, so C4 is a decision — ship voice in the default build, or grade C4 against an
opt-in build and say so. That is Sean's call and it is cheap either way; it is not a defect.

The genuine HIGH from that lane was elsewhere, and it was unlisted: the engine's own
missing-credential message tells the operator to select a model prefixed `ollama:` because no API
key is needed — and **doing exactly that reproduced the identical error**, because config
resolution returned `MissingApiKey` before the model string was read. The advertised credential-free
route was unreachable. Fixed at `9fe6ad86`. Same shape as the `[browser.policy]` HIGH.

---

## 5. Owed to Sean — nothing is blocked on him

- **core#254 reply** — drafted, precondition cleared, ready to post unchanged:
  `.planning/intel/CORE-254-REPLY-DRAFT.md`. Both fixes we took have landed.
- **Release trust root + signed manifest asset.** Doesn't block cutting an RC; blocks the RC
  updating itself.
- **Discord and Telegram vendor credentials** — the two adapters `24-03` designated the reference
  pair. Criterion 3 cannot close without them.
- **Phase 27 credentials, named exactly** — `FLUX_API_KEY` (C3 accounting); `GROQ_API_KEY` **or**
  `OPENAI_API_KEY` (C4 transcription — **there is no local STT path in the tree**);
  `OPENAI_API_KEY`/`ELEVENLABS_API_KEY` for TTS→barge-in. **Piper's local voices are the one
  credential-free route to a real interruption test**, and are worth taking for that reason alone.
- **The C4 voice decision** — default build or opt-in (§4b). Cheap either way, but it is a call.
- **Tag / publish**, and the **Desktop digest re-pin** on the same train
  (`CLASS-CONTRACT-01`; `observation.rs:329` makes a mismatch a hard error at `ready`).

---

## 6. Do first, next session

1. **Merge the four running lanes as they report**, verifying each rather than trusting it.
2. **Continue Phase 30 serially** — 30-02 → 30-03 → 30-04. 30-03 is where positioning is decided;
   30-01 and 30-02 deliberately decline to position.
3. **One contract regeneration over the merged tree** once the wave settles — never per-lane
   (`CLASS-CONTRACT-01`). Desktop must re-pin in the same train.
4. **Do not treat the 11 can-ship-open criteria as blocking.** That is precisely what turned
   Phase 20 into a 74-plan loop lasting two weeks.

**Verify what landed before redoing anything.** Six lanes were interrupted today and every one
was recoverable **because the worktree was checked before assuming.**
