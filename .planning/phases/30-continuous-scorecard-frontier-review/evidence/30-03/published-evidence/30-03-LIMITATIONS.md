# 30-03 — LIMITATIONS

**Rendered by `wayland-scorecard claims publish`. Do not edit.** Every dimension this phase could not measure, with its evidence explicitly unavailable and the exact substitution point that would change it. This is not a gap in the report; this IS the report.

- register sha256: `6e60102cf284c3115615bbca1176eb705c3d06fd2588445bd99689bf69cbadb3`
- limitations: **20**

## LIM-01

> The security dimension was not measured for wayland-core. The shared loopback meter records request body digests and per-leaf hashes, not bodies, so the byte-search extraction the frozen protocol specified was never possible. A narrower exact-leaf comparison was deliberately NOT substituted, because silently narrowing an extraction after the fact is how a protocol stops meaning what it said.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: meter_records_digests_not_bodies |
| references | `LEG-03-BLOCKER` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv |
| substitution point | Request-body retention under a redaction policy, or leaf-hash exposure, in crates/wcore-eval-scenarios/src/fixtures/openai.rs. A seam request is open; the change touches the shared meter and is release-coordinated. |

## LIM-02

> The security dimension was not measured for Hermes, for the same instrument reason as wayland-core: the shared meter records digests rather than bodies.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: meter_records_digests_not_bodies |
| references | `LEG-08-BLOCKER` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv |
| substitution point | The same meter seam as LIM-01. |

## LIM-03

> The security dimension was not measured for OpenClaw, for the same instrument reason as the other two tools.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: meter_records_digests_not_bodies |
| references | `LEG-13-BLOCKER` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv |
| substitution point | The same meter seam as LIM-01. |

## LIM-04

> Cognitive tax was not measured for wayland-core. The cross-audit panel ruled unanimously, before any trial ran, that the dimension is not measurable in a scripted tier at all. F30-03 is therefore incomplete on one of its five named dimensions.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: panel_ruled_not_measurable_in_this_tier |
| references | `LEG-05-BLOCKER` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv |
| substitution point | A human-subject or task-completion study outside the scripted tier. No fixture substitution can produce it, which is why the panel refused to proxy it. |

## LIM-05

> Cognitive tax was not measured for Hermes, on the same unanimous pre-trial panel finding.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: panel_ruled_not_measurable_in_this_tier |
| references | `LEG-10-BLOCKER` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv |
| substitution point | The same out-of-tier study as LIM-04. |

## LIM-06

> Cognitive tax was not measured for OpenClaw, on the same unanimous pre-trial panel finding.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: panel_ruled_not_measurable_in_this_tier |
| references | `LEG-15-BLOCKER` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/legs.tsv |
| substitution point | The same out-of-tier study as LIM-04. |

## LIM-07

> The shipped command `init` is owned by no CTRL-01 coverage family, so it has no security authority owner, no recorded maturity, no evidence IDs and no peer baseline. It is unreviewed surface, and it is a first-run credential-adjacent path.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: surface_owned_by_no_family |
| references | `SURFACE-INVENTORY` → .planning/phases/30-continuous-scorecard-frontier-review/30-01-SURFACE-INVENTORY.md |
| substitution point | Assignment of the command to a coverage family in CTRL-01, which is the owning phase's action and not a review's. |

## LIM-08

> The shipped command `setup` is owned by no CTRL-01 coverage family. It is unreviewed surface, and it is a first-run credential-adjacent path.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: surface_owned_by_no_family |
| references | `SURFACE-INVENTORY` → .planning/phases/30-continuous-scorecard-frontier-review/30-01-SURFACE-INVENTORY.md |
| substitution point | Assignment of the command to a coverage family in CTRL-01. |

## LIM-09

> The shipped command `profile` is owned by no CTRL-01 coverage family. It is unreviewed surface, and it is a first-run credential-adjacent path.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: surface_owned_by_no_family |
| references | `SURFACE-INVENTORY` → .planning/phases/30-continuous-scorecard-frontier-review/30-01-SURFACE-INVENTORY.md |
| substitution point | Assignment of the command to a coverage family in CTRL-01. |

## LIM-10

> The shipped command `mcp-serve` is owned by no CTRL-01 coverage family, so it carries no security authority owner and no recorded maturity.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: surface_owned_by_no_family |
| references | `SURFACE-INVENTORY` → .planning/phases/30-continuous-scorecard-frontier-review/30-01-SURFACE-INVENTORY.md |
| substitution point | Assignment of the command to a coverage family in CTRL-01. |

## LIM-11

> The shipped command `models` is owned by no CTRL-01 coverage family, so it carries no security authority owner and no recorded maturity.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: surface_owned_by_no_family |
| references | `SURFACE-INVENTORY` → .planning/phases/30-continuous-scorecard-frontier-review/30-01-SURFACE-INVENTORY.md |
| substitution point | Assignment of the command to a coverage family in CTRL-01. |

## LIM-12

> The shipped command `project-context` is owned by no CTRL-01 coverage family, so it carries no security authority owner and no recorded maturity. Across the six unowned commands there are fifteen unreviewed surface rows in total.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: surface_owned_by_no_family |
| references | `SURFACE-INVENTORY` → .planning/phases/30-continuous-scorecard-frontier-review/30-01-SURFACE-INVENTORY.md |
| substitution point | Assignment of the command to a coverage family in CTRL-01. |

## LIM-13

> The evidence ID PEER-PROBE-2026-07-26 names no openable artifact anywhere in the repository. It describes a method rather than an object, and no captured output exists. Six coverage families cite it, and in each of them it carries roughly half the Delta column, so every probe finding in those families is uncheckable by a reader. This is why no claim in this phase rests on it.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: evidence_id_names_no_openable_artifact |
| references | `ID-RESOLUTION` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/evidence-id-resolution.tsv |
| substitution point | Re-running the structural probes against both peer trees at the BASE-2026-07-13 commits and committing the captured output under a named evidence ID. |

## LIM-14

> The evidence ID F05-TRUTH-{n} is a template rather than an instance, so it does not mechanically resolve. Its concrete uses in family rows do resolve for a reader who cross-references the row above them.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: template_id_not_an_instance |
| references | `ID-RESOLUTION` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/evidence-id-resolution.tsv |
| substitution point | Expanding the template into its concrete instances in CTRL-01's evidence index. |

## LIM-15

> Every measurement in this phase was taken against a scripted loopback fixture that holds the model constant by construction. Nothing here says anything about model quality, real-world task success, or dollar cost at provider rates. No claim about real-world use is publishable from this evidence, and the checker refuses every attempt at one.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: no_live_provider_measurement_exists_in_this_phase |
| references | `PROTOCOL` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-02/protocol.json |
| substitution point | A live-provider tier run with real credentials against real provider endpoints. That requires a credential only Sean can supply, and no gate in this phase may be passed by supplying one. |

## LIM-16

> The peer delta truth is recorded UNPROVEN on all one hundred and forty-eight surface rows, unchanged by this phase. 30-02's comparatives are dimension-level rather than surface-level, so nothing in it produced a per-surface peer comparison.

| field | value |
|---|---|
| scope | `STATIC_SOURCE` |
| evidence | UNAVAILABLE: no_per_surface_peer_comparison_has_run |
| references | `SURFACE-TRUTHS` → .planning/phases/30-continuous-scorecard-frontier-review/evidence/30-01/surface-truths.tsv |
| substitution point | A per-surface comparative pass against both pinned peer trees, which no plan in this phase performs. |

## LIM-17

> Real dollar cost at provider rates was not measured. The cost dimension was metered in synthetic fixture units against a scripted script, and those units do not convert to money.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: no_real_provider_billing_was_observed |
| references | `TRIAL-RESULTS` → .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md |
| substitution point | A live-provider tier run with real credentials and real billing records. Requires a credential this phase may not use. |

## LIM-18

> wayland-core did not start on the headless trial host until an encrypted-file credentials configuration and a vault passphrase were supplied, reporting that no OS keyring was usable. Neither reference tool required an equivalent step. Whether the remedy for this is effective is being tested elsewhere and is NOT settled here, so nothing in this phase positions on it.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: remedy_under_test_by_another_lane_result_not_in |
| references | `TRIAL-RESULTS` → .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md |
| substitution point | The concurrent lane's result on whether the keyring remedy actually works on a headless host, then a re-run of the startup leg at that commit. |

## LIM-19

> The fixture is FIFO-cursored and is the same dialect wayland-core's own contract tests were built against. All four panel members named this bias independently. The frozen protocol bounds it by treating a 409 as HARNESS_INCOMPATIBLE, which is neither success nor failure, but no trial ever triggered that state, so the bound was never exercised and the bias remains open.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: bound_never_exercised_by_any_trial |
| references | `TRIAL-RESULTS` → .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md |
| substitution point | Content-routed rather than FIFO-cursored fixture matching, so a tool whose request order differs is not penalised. This is the second half of the open meter seam request. |

## LIM-20

> The correctness and recovery comparatives are not yet measurable. They ran and produced real numbers, but the canonical script speaks one tool's dialect and two of the three harnesses could not execute it, so the numbers do not measure the dimension they are named after. This phase therefore publishes no correctness or recovery comparison in either direction, including none favourable to wayland-core.

| field | value |
|---|---|
| scope | `SCRIPTED_HARNESS` |
| evidence | UNAVAILABLE: measurement_confounded_by_script_dialect |
| references | `TRIAL-RESULTS` → .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md |
| substitution point | Per-tool dialect compilation of the canonical script, a re-pre-registered protocol carrying it, and a full re-run of the nine RUN legs. |

## Confounded legs — measured, but not measuring

These legs RAN and their numbers are real. They are recorded here because the number does not measure the thing its dimension is named after, so no directional claim may rest on one — the checker refuses it by rule `directional_claim_on_confounded_leg`. This is a stronger statement than UNPROVEN: an unproven leg produced nothing, whereas a confounded leg produced something that would be READ WRONGLY.

| leg | defect | evidence | substitution point |
|---|---|---|---|
| `LEG-01` | The single canonical script emits a tool call named `write_file`, a name only Hermes exposes; Wayland's equivalent is named `Write`. OpenClaw scored 0/30 on the identical script, so two of three harnesses failed it. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | Per-tool dialect compilation of the canonical script (prescribed by panel member codex and not adopted by the frozen protocol), then a re-run under an amended and re-pre-registered protocol. |
| `LEG-02` | Same canonical-script dialect defect as LEG-01: recovery was scored from the same `write_file` script that two of three harnesses could not execute. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | Per-tool dialect compilation, then a re-run under an amended and re-pre-registered protocol. |
| `LEG-04` | Cost was metered against a script two of the three tools could not complete. All three spent an identical 20.00 units, so equal spend here represents unequal work and does not support a cost comparison. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | A cost measurement taken over a task all three tools actually complete, so that spend is comparable per unit of work delivered. |
| `LEG-06` | Hermes' 30/30 was scored on a script written in Hermes' own tool dialect, which is the confound rather than a Hermes defect. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | Per-tool dialect compilation, then a re-run under an amended and re-pre-registered protocol. |
| `LEG-07` | Same dialect confound as LEG-06, on the recovery dimension. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | Per-tool dialect compilation, then a re-run under an amended and re-pre-registered protocol. |
| `LEG-09` | Cost metered against a script only this tool could complete; equal spend represents unequal work. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | A cost measurement over a task all three tools complete. |
| `LEG-11` | OpenClaw scored 0/30 on the same `write_file` script. Two of three harnesses failing one script is evidence about the script's dialect, not about two products. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | Per-tool dialect compilation, then a re-run under an amended and re-pre-registered protocol. |
| `LEG-12` | Same dialect confound as LEG-11, on the recovery dimension. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | Per-tool dialect compilation, then a re-run under an amended and re-pre-registered protocol. |
| `LEG-14` | Cost metered against a script this tool could not complete; equal spend represents unequal work. | .planning/phases/30-continuous-scorecard-frontier-review/30-02-TRIAL-RESULTS.md | A cost measurement over a task all three tools complete. |

