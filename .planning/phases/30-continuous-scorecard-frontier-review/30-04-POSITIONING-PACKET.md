# 30-04 — Positioning packet: the evidence, and not the decision

> **This document deliberately contains no recommendation, no readiness statement and no market
> comparison. The omission is the requirement, not an oversight.**
>
> Phase 30's Criterion 4 reserves frontier positioning to Sean. A packet that ended with "and
> therefore Wayland is ready for X" would be the exact violation the phase was built to prevent
> — and it would be worse than the phase declaring itself not achieved. So this document
> assembles what may be claimed, what may not, what is unproven, what it would cost to prove,
> and what is missing only a real key. It stops there.
>
> Machine-readable index: `evidence/30-04/packet-index.tsv`, gate-checked free of readiness
> vocabulary.

Assembled at `32cc7ac8f64d43d9c7d937c105013c13e9bbce95`, lane `lane/30-04`.

---

## 0. The one-line shape of the evidence

**Nine claims may be published. None of them is a peer comparison.** Not a favourable one, not
an unfavourable one, not a hedged one. Twenty limitations are published alongside them. Zero of
the five comparative dimensions yielded a usable result.

---

## 1. What may be claimed — 9, as published

Source: `30-03-CLAIMS-ALLOWED.md`. Every one carries an evidence pointer that must resolve, is
scope-contained, and is classified consistently — enforced by code, not by review.

| # | Substance |
|---|---|
| ALW-01 | The comparative methodology was pre-registered before any measurement existed, proved from history at commit `a7bd5d87`. |
| ALW-02 | Both peer pins re-resolve at their declared versions: Hermes 0.17.0 @ `dbe734be`, OpenClaw 2026.6.2 @ `11a0ad10`. |
| ALW-03 | All three tools were provisioned from their own lockfiles at their own pinned commits. |
| ALW-04 | The fifteen-leg accounting is complete and closed: 9 RUN, 6 UNPROVEN, each accounted exactly once. |
| ALW-05 | CTRL-01's ten coverage families satisfy all seven declared clauses, zero defects. |
| ALW-06 | 39 of 42 ledger evidence IDs resolve to concrete, openable objects. |
| ALW-07 | Thirteen tracking claims are falsified by the tree, and **every one of them understates the program**. |
| ALW-08 | Two of the three harnesses scored zero of thirty on the identical canonical script, which emits a tool call named `write_file`. |
| ALW-09 | 30-02's verifier is demonstrably able to fail. |

**ALW-08 is the shape every claim in this phase had to take**: factual, non-directional, about
what was *run* rather than about which product is better.

## 2. What may NOT be claimed — 10 attempted and refused

Source: `30-03-CLAIMS-PROHIBITED.md`. Each names the rule that refused it. Two are worth
reading closely, because they are the ones a reader will expect to find and will not:

- **`Hermes completed the scripted task more reliably than wayland-core.`** Well-formed,
  bounded, correctly scoped — and refused, by `confounded_leg_supports_no_comparison`. This is
  the *unflattering* claim. It was refused for the same reason a flattering one would be: the
  number does not measure the dimension it is named after.
- **`Cost is practically indistinguishable across the three tools.`** Refused on the same
  ground. All three spent an identical 20.00 units, but two completed 0/30 of the task. Equal
  spend for unequal work is not equivalence, and on this data the equivalence claim is the more
  misleading of the two.

The register refuses comparisons **in both directions**. Nothing here was withheld because it
was unflattering, and nothing was published because it was.

## 3. What is unproven, and what it would cost to close

| Leg | State | What would close it |
|---|---|---|
| security × 3 tools | UNPROVEN — the shared meter records body digests and per-leaf hashes, never bodies, so the frozen canary byte-search was never performable. A narrower extraction was deliberately **not** substituted. | Request-body retention under a redaction policy, or leaf-hash exposure, in `crates/wcore-eval-scenarios/src/fixtures/openai.rs`. Release-coordinated; seam request open. |
| cognitive tax × 3 tools | UNPROVEN — all four panel members independently refused to proxy it, **before any trial ran**. | A human-subject or task-completion study outside the scripted tier. No fixture substitution can produce it. |
| correctness / recovery / cost × 3 | RUN but **CONFOUNDED** — the canonical script speaks one tool's dialect. | Per-tool dialect compilation, a re-pre-registered protocol v2 carrying committed translation digests, and a full re-run of all nine legs. **This is a new pre-registration, not an amendment.** |
| `peer_delta` on 148/148 surfaces | UNPROVEN — measured as unproven, not assumed. 30-02's comparatives are dimension-level, not surface-level. | A per-surface comparative pass against both pinned peer trees. No plan in this phase performs one. |
| `operator_completeness` on 148/148 | UNPROVEN — a command-tree walk cannot observe an operator journey. | A three-platform operator journey. |
| 15 surface rows / 6 top-level commands | Owned by no coverage family — no security owner, no maturity, no evidence, no peer baseline. Three (`setup`, `init`, `profile`) are credential-adjacent. | The row owners adopting them into CTRL-01. |
| hidden clap aliases | Unmeasured. `forgeflows` runs live and has no inventory row. | Alias extraction from the clap definitions — which trades away the "truth-from-the-artifact" property, so it is a deliberate trade rather than a silent fix. |
| macOS / Windows command trees | Unmeasured. Everything in this phase is Linux. | Running the walk on the other two platforms. Phase 24 already measured a case where the macOS binary provably did not carry code the Linux one did. |

## 4. Every real-key and real-account limit, in one place

Full register with substitution points: `evidence/30-04/real-key-limits.tsv` — **8 entries,
none graded as met.** Summary:

| | Missing input | Substitution point |
|---|---|---|
| KEY-01 | Sean's Ed25519 approval public key | `reserved_authority.rs:APPROVAL_ROOT_PUBKEY_HEX` |
| KEY-02 | A live provider credential (security dimension) | `fixtures/openai.rs` |
| KEY-03 | Real provider billing (dollar cost) | a live-provider tier run |
| KEY-04 | A GitHub API credential (PR / issue / release / deployment observability) | Sean's own view of the remote |
| KEY-05 | Phase 28's inherited limits | `28-04-PHASE-VERDICT.md` |
| KEY-06 | Phase 29's inherited limits | `29-PHASE-VERDICT.md` |
| KEY-07 | A second-host SSH authorisation | only Sean can mint it |
| KEY-08 | Per-tool dialect compilation — **needs no credential**, only a re-pre-registration | protocol v2 |

**KEY-08 is the cheapest and the most consequential.** It is the only entry on this list that
requires no key, no account and no authorisation from anybody — and without it Criterion 2
cannot be re-graded at all.

## 5. The state of the four Success Criteria

Graded verbatim in `30-PHASE-VERDICT.md`, machine-verified through the shipped
`wayland-scorecard verify`.

| | Grade |
|---|---|
| Criterion 1 — per-surface truth refreshed at each phase | **NOT MET** |
| Criterion 2 — five-dimension trials across three tools | **NOT MET** |
| Criterion 3 — published claims match evidence, no unsupported superiority | **MET WITH STATED EXCEPTIONS** |
| Criterion 4 — no reserved action without Sean's approval | **PARTIAL** |

## 6. What this phase inherits, unresolved

- **Phase 28** graded its own Criterion 4 **NOT MET**; its acceptance gate did **not** pass, with
  `zero_undispositioned_findings=false` and `zero_unresolved_critical_or_high=false`. Its
  certification covers commit `32e2f57d`, **not** the current integration tip.
- **Phase 29** graded its own goal **NOT ACHIEVED**, all four criteria PARTIAL. `cargo deny`'s
  real verdict is `FAIL, exit 5`. The runtime plugin and backend trust roots are still all-zeros
  placeholders.
- **Phases 21, 22, 23A, 23B, 24, 25, 26, 27** all carry open requirements with named unmet
  clauses. Phases 21, 22, 23B and 27 each graded their own goal NOT ACHIEVED.
- **Amendment A3 binds**: a certification receipt may assert exactly three things and may **not**
  assert "zero known defects". Tonight alone added a HIGH silent inbound message-loss defect
  (fixed), a HIGH headless-remedy defect (fixed, `769d98b3`), and two MEDIUMs from the Phase 28
  adjudication (in flight). **Nothing in this packet implies a clean sheet.**

## 7. What this packet does not do

- It does not recommend a position, in any direction.
- It does not state whether anything is ready for anything.
- It does not compare Wayland to a market, a competitor, or a category.
- It does not tell Sean what to conclude.

Those are Criterion 4's, and Criterion 4 is his.
