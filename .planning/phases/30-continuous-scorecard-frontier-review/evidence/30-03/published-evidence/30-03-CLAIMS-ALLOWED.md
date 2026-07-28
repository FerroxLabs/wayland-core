# 30-03 — Claims ALLOWED

**Rendered by `wayland-scorecard claims publish`. Do not edit.** Every sentence below is rendered from a register entry that survived the checker. A sentence added here by hand fails the on-hardware re-render diff.

- register sha256: `6e60102cf284c3115615bbca1176eb705c3d06fd2588445bd99689bf69cbadb3`
- allowed claims: **9**
- tie band: `0.05`

The allowed set is whatever the evidence supports. It is SMALL, and that is the honest shape of this phase's evidence rather than a shortfall.

## ALW-01 — factual

> 30-02's comparative protocol was content-addressed and committed before any measurement of any kind existed: the pre-registration commit a7bd5d87 contains no measurement, the results commit abf652af is a distinct later commit, and git merge-base --is-ancestor confirms the ordering.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: ordering_is_a_history_fact_not_a_sampled_measurement |
| evidence | `PROTOCOL-DIGEST` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/protocol.sha256 |

## ALW-02 — factual

> Both pinned peer baselines re-resolve exactly under an independent read: Hermes at dbe734be reports version 0.17.0 and OpenClaw at 11a0ad10 reports 2026.6.2, matching what CTRL-01 records, with no write verb used in either reference tree.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: pin_resolution_is_exact_not_estimated |
| evidence | `PEER-BASELINES` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/peer-baselines.txt |

## ALW-03 — factual

> All three tools were provisioned at their own pinned commits from their own committed lockfiles. No HEAD snapshot, no registry build and no substitute version was used for any of them.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: provisioning_is_a_recorded_procedure_not_a_measurement |
| evidence | `PEER-CLONES` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/peer-clones.txt |

## ALW-04 — factual

> 30-02's trial accounting is complete and closed: fifteen legs, nine recorded RUN and six recorded UNPROVEN, with every leg naming a capture file that exists. No leg is absent and none is recorded as skipped.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: an_accounting_count_is_a_census |
| evidence | `LEG-ACCOUNTING` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv |

## ALW-05 — factual

> CTRL-01's ten coverage families each satisfy all seven of its own declared required clauses, with zero clause defects, zero undeclared evidence IDs and zero unpinned peer baselines.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: a_clause_check_is_a_census |
| evidence | `FAMILY-CLAUSES` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/family-clause-check.txt |

## ALW-06 — factual

> Thirty-nine of CTRL-01's forty-two declared evidence IDs resolve to concrete objects; one is PARTIAL because its citation omits a directory, and two do not resolve at all.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: a_resolution_count_is_a_census |
| evidence | `ID-RESOLUTION` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/evidence-id-resolution.tsv |

## ALW-07 — factual

> Thirteen claims the tracking documents make are falsified by the tree, and every one of the thirteen understates what has landed. Two deliberate control claims were re-checked in the same run and both correctly held.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: a_falsification_count_is_a_census |
| evidence | `STALENESS` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/staleness-check.txt |

## ALW-08 — factual

> Two of the three harnesses scored zero of thirty on the identical canonical script, which emits a tool call named write_file. That is a property of the script that was run, and it is recorded here because it is what the correctness and recovery numbers actually reflect.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: a_statement_about_which_script_ran_is_not_a_sampled_quantity |
| evidence | `WAYLAND-CORRECTNESS` → LEG-01 in .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv; `OPENCLAW-CORRECTNESS` → LEG-11 in .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv |

## ALW-09 — factual

> 30-02's trial verifier is able to fail. Run against the shipped release binary it produced one pass and four distinct refusals: a one-byte protocol mutation, a dropped leg, an UNPROVEN leg with its blocker removed, and an invented scope tag rejected at deserialization.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| peer baseline | n/a |
| bounds | UNAVAILABLE: a_refusal_count_is_a_census |
| evidence | `VERIFIER-KNOWN-GOOD-BAD` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/verifier-known-good-bad.txt |

