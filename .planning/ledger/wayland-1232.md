---
issue: 1232
repo: FerroxLabs/wayland
kind: defect
title: "Static arms for two open-weights passthrough families served at wildly different limits by different hosts"
status: open
last_verified_commit: 1798076f
criteria:
  - id: c1
    text: "The claim in passthrough.rs's module docs matches the table's contents"
    state: met
    evidence: "absent:crates/wcore-config/src/limits/passthrough.rs::No open-weights family is listed here"
    owner: core
    note: "CLOSED by the wayland#1176 c5 work and verified here at 1798076fe rather than inherited. The false sentence the ticket quotes -- 'No open-weights family is listed here: those live in CATALOGUE_CEILINGS, where the same rule already applies' -- is gone: grep for it returns nothing, while the known-positive control in the same call, grep -c 'open-weights' on the identical path, returns 5. So the query reads the file and the absence is real. What stands in its place is honest about the same fact: passthrough.rs:49 'OPEN WEIGHTS, STATED HONESTLY. This module used to claim \"no open-weights', :51 'and M3 rows and the DeepSeek V4 rows below are open-weights ids, served by', and :68 'Twelve of the rows below are open-weights ids. On the 2026-08-30 pull SEVEN'. The twelve-row count and the seven-violation count both match what the live gate printed today. The evidence token is an absent: anchor deliberately -- it is the only form that goes red if the claim is ever reinstated."
  - id: c2
    text: "For each of the twelve MiniMax/DeepSeek rows, a recorded decision: either the arm is removed (and the omit-max-tokens consequence stated and measured), or it is kept with the host spread written down and the reason it is safe despite the spread"
    state: not-met
    owner: core
    note: "NOT MET, and the distinction being refused here is between DEBT and a DECISION. What exists is OPEN_WEIGHTS_ARM_DEBT (scripts/check-model-limits-freshness.py:227-235): seven of the twelve ids listed, each dated 2026-11-30 and owned by gh#1232, which is this ticket. A dated debt entry says 'somebody must decide this by November'; it is not the decision, and the criterion asks for the decision. The other five rows -- deepseek-v4-flash-vision-exp, deepseek-v4-pro-0813, minimax-m2.5-highspeed, minimax-m2.7, minimax-m2.7-highspeed -- are not violations (their hosts agree), which is a measurement rather than a decision too, though for those the criterion is arguably discharged by the spread being written down. MEASURED LIVE 2026-08-30 on hetzner-dsm, exit 0 PASS with the REPORT block naming all seven, verbatim for one of them: 'deepseek-v4-pro is an OPEN-WEIGHTS id (deepseek) with a STATIC ARM recorded at context 1000000, while its hosts serve it at 128000 to 1050000 (8.2x) across 64 endpoints', and the others at deepseek-v4-flash 8.0x/61, deepseek-v4-flash-0731 5.1x/35, minimax-m2 5.1x/19, minimax-m2.1 5.1x/24, minimax-m2.5 3.5x/44, minimax-m3 4.0x/43. WHY IT IS NOT A QUICK FIX, from the ticket and independently true: an arm REVOKES should_omit_max_tokens (engine.rs, the wire field is omitted only when model_output_ceiling(...).is_none()), so deleting one restores the provider's own natural ceiling and deleting the wrong one CUTS real output. wayland#1176 c1's every_passthrough_vendor_model_resolves_its_arm asserts each arm, so the removal and that test move together."
  - id: c3
    text: "Whatever is decided is enforced by a test that fails if a later change reverses it -- not by a comment"
    state: not-met
    owner: core
    note: "NOT MET, because c2 is not met: there is no decision yet for a test to enforce. Recording what already exists so this is not re-derived. The FORWARD direction is enforced and was red-armed by this lane on 2026-08-30: scan_open_weights_arms reads PASSTHROUGH_VENDOR_MODELS on every run and measures host spread over every provider, so a NEW host-variable open-weights arm fails the release the day it lands. Proof, not assertion -- the live gate was exit 0 PASS, then the minimax-m2.5 row at check-model-limits-freshness.py:233 (a dict literal, printed before and after, and distinguished from the two self-test fixtures at :1030-1031 that share the same id) was deleted and the file touched; the gate went to exit 1 with 'FAIL -- 1 model(s) we CLAIM to cover are missing or over-claimed' and named 'minimax-m2.5 ... It is not listed in OPEN_WEIGHTS_ARM_DEBT, so it is a NEW violation.' Exactly one model, so it discriminates. Restored, touched, exit 0 and PASS again. NOTE FOR WHOEVER TAKES THIS: --self-test is a VACUOUS instrument for that property -- the same mutation leaves it exit 0, because it injects DEBT_OK/DEBT_OLD fixtures and never reads the real dict. Use the live gate."
---

Created 2026-08-30 by the 0.13.12 re-grade lane. This open, in-scope issue had NO ledger
file, so `scripts/check-criteria-ledger.py` was reporting it as a COVERAGE gap and nothing
graded it — while two other tickets were simultaneously blocking the release on the work it
owns.

**This ticket is the SINGLE OWNER of the twelve MiniMax/DeepSeek static arms.** Three
tickets contended for that one piece of work and the contention is resolved here:

| ticket | disposition |
|---|---|
| `wayland#1176` c5 | forward direction closed and re-verified; state corrected `not-met` → `superseded`, naming this ticket. It had a `handoff:` already, but `not-met` + `owner: core` is counted OUTSTANDING by the readiness gate whatever the handoff says, so the handoff was decorative and #1176 went on blocking for work it had handed away. |
| `wayland#1214` c1 | `superseded` onto this ticket. Its c1 and this c2 are the same work on the same twelve rows; this one states the `should_omit_max_tokens` consequence, which #1214's wording does not. #1214 c2/c3/c4 are separately closed — they were already discharged by the #1176 c5 work and nobody had re-graded them. |
| `wayland#1232` (this) | owner. c1 met, c2 and c3 outstanding. |

Criteria are the ticket's own Acceptance wording, transcribed verbatim.

`kind: defect` is deliberate and is not the safe-direction default: the arms are a live
over-claim in the `#165` direction. `model_output_ceiling` is keyed on the model id alone —
its `_provider` parameter is underscore-prefixed and unused — so a user pointing
`--model deepseek-v4-pro` at a host serving it at 128,000 is sized against 1,000,000.
