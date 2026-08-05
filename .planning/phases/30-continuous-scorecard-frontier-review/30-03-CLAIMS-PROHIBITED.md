# 30-03 — Claims PROHIBITED

**Generated from the checker's refusal set by `wayland-scorecard claims publish`. Do not edit.** These are not promises about what we will avoid saying — a hand-written list of those is worth nothing. Each entry below is a claim this program actually attempted and the checker refused, with the rule that refused it and the evidence it lacked.

- register sha256: `6e60102cf284c3115615bbca1176eb705c3d06fd2588445bd99689bf69cbadb3`
- refused claims: **10**

## ATT-01 — REFUSED by `confounded_leg_supports_no_comparison`

> wayland-core's cost is practically indistinguishable from both peers.

| field | value |
|---|---|
| declared class | `comparative` |
| declared scope | `SCRIPTED_HARNESS` |
| rule | `confounded_leg_supports_no_comparison` |
| what it lacked | an unconfounded measurement (`LEG-04` carries a recorded instrument defect) |
| refusal | claim `ATT-01` compares (declared comparative) on `LEG-04`, whose measurement 30-02 recorded as confounded by an instrument defect: Cost was metered against a script two of the three tools could not complete. All three spent an identical 20.00 units, so equal spend here represents unequal work and does not support a cost comparison. |

## ATT-02 — REFUSED by `directional_on_interval_containing_zero`

> wayland-core is ahead of OpenClaw on scripted correctness.

| field | value |
|---|---|
| declared class | `comparative` |
| declared scope | `STATIC_SOURCE` |
| rule | `directional_on_interval_containing_zero` |
| what it lacked | separation from zero (interval [-0.1135, 0.1135] contains it) |
| refusal | claim `ATT-02` asserts a direction (`ahead`) on delta interval [-0.1135, 0.1135], which frontier_trials::direction_for entails `INCONCLUSIVE` rather than a direction |

## ATT-03 — REFUSED by `evidence_id_unresolved`

> Neither peer ships an SBOM at baseline, so wayland-core's SBOM requirement has no counterpart to match and is therefore a supply-chain lead.

| field | value |
|---|---|
| declared class | `comparative` |
| declared scope | `STATIC_SOURCE` |
| rule | `evidence_id_unresolved` |
| what it lacked | a resolvable citation (`PEER-PROBE-2026-07-26` is UNRESOLVED) |
| refusal | claim `ATT-03` cites CTRL-01 evidence ID `PEER-PROBE-2026-07-26`, which 30-01 recorded `UNRESOLVED`: ids/PEER-PROBE-2026-07-26.txt |

## ATT-04 — REFUSED by `evidence_leg_unproven`

> No canary value left the harness during the security trials, so wayland-core did not exfiltrate secrets.

| field | value |
|---|---|
| declared class | `factual` |
| declared scope | `SCRIPTED_HARNESS` |
| rule | `evidence_leg_unproven` |
| what it lacked | a RUN leg (`LEG-03` is UNPROVEN) |
| refusal | claim `ATT-04` rests on `LEG-03`, which 30-02 recorded UNPROVEN: blockers/wayland-security.txt |

## ATT-05 — REFUSED by `scope_not_contained`

> In real-world deployments wayland-core starts cleanly on a headless host without additional credential configuration.

| field | value |
|---|---|
| declared class | `factual` |
| declared scope | `LIVE_PROVIDER` |
| rule | `scope_not_contained` |
| what it lacked | evidence at its own scope (it has only `STATIC_SOURCE`) |
| refusal | claim `ATT-05` is scoped `LIVE_PROVIDER` but cites `.planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md` gathered at `STATIC_SOURCE`, which does not contain it |

## ATT-06 — REFUSED by `unbounded_superiority`

> wayland-core is architecturally superior to both peers on sandbox and egress.

| field | value |
|---|---|
| declared class | `comparative` |
| declared scope | `STATIC_SOURCE` |
| rule | `unbounded_superiority` |
| what it lacked | either an interval or an explicit unproven-qualifier |
| refusal | comparative claim `ATT-06` asserts superiority (`superior`) from a STATIC_SOURCE census with neither an interval nor an explicit unproven-qualifier |

## ATT-07 — REFUSED by `misclassification`

> wayland-core recovers from induced failures better than Hermes.

| field | value |
|---|---|
| declared class | `factual` |
| declared scope | `SCRIPTED_HARNESS` |
| rule | `misclassification` |
| what it lacked | a class consistent with its own text (`better`) |
| refusal | claim `ATT-07` is declared `factual` but its text is comparative (`better`); relabelling does not dodge the classifier |

## ATT-08 — REFUSED by `unbounded_superiority`

> This is wayland-core's clearest unique capability.

| field | value |
|---|---|
| declared class | `comparative` |
| declared scope | `STATIC_SOURCE` |
| rule | `unbounded_superiority` |
| what it lacked | either an interval or an explicit unproven-qualifier |
| refusal | comparative claim `ATT-08` asserts superiority (`clearest`) from a STATIC_SOURCE census with neither an interval nor an explicit unproven-qualifier |

## ATT-09 — REFUSED by `comparative_without_pinned_baseline`

> wayland-core leads on autonomous coding.

| field | value |
|---|---|
| declared class | `comparative` |
| declared scope | `STATIC_SOURCE` |
| rule | `comparative_without_pinned_baseline` |
| what it lacked | a pinned peer baseline token |
| refusal | comparative claim `ATT-09` names no pinned peer baseline |

## ATT-10 — REFUSED by `no_evidence_reference`

> wayland-core is certified with zero known defects across every supported platform.

| field | value |
|---|---|
| declared class | `factual` |
| declared scope | `STATIC_SOURCE` |
| rule | `no_evidence_reference` |
| what it lacked | any evidence reference at all |
| refusal | claim `ATT-10` carries no evidence reference |

