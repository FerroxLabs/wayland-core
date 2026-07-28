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

## 2. Running right now — four lanes

| Lane | Doing |
|---|---|
| `lane/30-02` | Phase 30 trial protocol. Owns `peer_delta`, **UNPROVEN on all 148 surface rows** |
| `lane/24-c3-h2` | The installed gateway **cannot receive inbound at all** — build it or refuse loudly |
| `lane/27-gaps` | Phase 27, the weakest phase and the largest genuine parity gap |
| `lane/22-c3` | Phase 22 Criterion 3 — "one loop owner", **never attempted in three passes** |

Merge each as it reports, then continue Phase 30 serially: **30-02 → 30-03 → 30-04.**

---

## 3. Open HIGHs

| ID | What |
|---|---|
| `F24-C3-H2` | `run_gateway` builds no `InboundSubscriber` and no webhook host. Config says `enabled = true`; `rc=7`, nothing listening. Being fixed. |
| `F-28-02-002` | Stale AppContainer lease = DoS. **OPEN at HIGH by choice** — 28-04 declined a MEDIUM re-score that a literal reading permitted, because the downgrade opens the accept path and passes the gate. |
| `F29-02-H1` | `.cargo/audit.toml` silences RUSTSEC on a stated "sole path"; the graph has three. |
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
- **The instrument that hunts a defect class tends to carry it — now eight times.** Run every
  checker against a known-positive *and* a known-negative before trusting it.

---

## 5. Owed to Sean — nothing is blocked on him

- **core#254 reply** — drafted, precondition cleared, ready to post unchanged:
  `.planning/intel/CORE-254-REPLY-DRAFT.md`. Both fixes we took have landed.
- **Release trust root + signed manifest asset.** Doesn't block cutting an RC; blocks the RC
  updating itself.
- **Discord and Telegram vendor credentials** — the two adapters `24-03` designated the reference
  pair. Criterion 3 cannot close without them.
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
